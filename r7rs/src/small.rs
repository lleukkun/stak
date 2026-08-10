mod error;
mod primitive;

pub use self::error::Error;
use self::primitive::Primitive;
use core::ops::{Add, Mul, Sub};
use stak_device::{Device, DevicePrimitiveSet};
use stak_file::{FilePrimitiveSet, FileSystem};
use stak_inexact::InexactPrimitiveSet;
use stak_native::{
    ArithmeticPrimitiveSet, EqualPrimitiveSet, ListPrimitiveSet, TypeCheckPrimitiveSet,
};
use stak_process_context::{ProcessContext, ProcessContextPrimitiveSet};
use stak_time::{Clock, TimePrimitiveSet};
use stak_vm::{
    Cons, Heap, ListVectorCursor, Memory, Number, PrimitiveSet, Tag, Type, Value, list_vector_cell,
};
use winter_maybe_async::{maybe_async, maybe_await};

const BYTEVECTOR_TAG: Tag = 8;
const STRING_TAG: Tag = 5;
const VECTOR_FACTOR: usize = 64;
const UTF8_BUFFER_SIZE: usize = 512;
const UTF8_REPLACEMENT_CODE_POINT: u32 = 0xfffd;

/// A primitive set that covers [the R7RS small](https://standards.scheme.org/corrected-r7rs/r7rs.html).
pub struct SmallPrimitiveSet<D: Device, F: FileSystem, P: ProcessContext, C: Clock> {
    device: DevicePrimitiveSet<D>,
    file: FilePrimitiveSet<F>,
    process_context: ProcessContextPrimitiveSet<P>,
    time: TimePrimitiveSet<C>,
    inexact: InexactPrimitiveSet,
    equal: EqualPrimitiveSet,
    arithmetic: ArithmeticPrimitiveSet,
    type_check: TypeCheckPrimitiveSet,
    list: ListPrimitiveSet,
}

impl<D: Device, F: FileSystem, P: ProcessContext, C: Clock> SmallPrimitiveSet<D, F, P, C> {
    /// Creates a primitive set.
    pub fn new(device: D, file_system: F, process_context: P, clock: C) -> Self {
        Self {
            device: DevicePrimitiveSet::new(device),
            file: FilePrimitiveSet::new(file_system),
            process_context: ProcessContextPrimitiveSet::new(process_context),
            time: TimePrimitiveSet::new(clock),
            inexact: Default::default(),
            equal: Default::default(),
            arithmetic: Default::default(),
            type_check: Default::default(),
            list: Default::default(),
        }
    }

    /// Returns a reference to a device.
    pub fn device(&self) -> &D {
        self.device.device()
    }

    /// Returns a mutable reference to a device.
    pub fn device_mut(&mut self) -> &mut D {
        self.device.device_mut()
    }

    #[inline(always)]
    fn operate_comparison<H: Heap>(
        memory: &mut Memory<H>,
        operate: fn(Number, Number) -> bool,
    ) -> Result<(), Error> {
        let [x, y] = memory.pop_numbers()?;

        memory.push(memory.boolean(operate(x, y))?.into())?;
        Ok(())
    }

    #[inline]
    fn rib<H: Heap>(memory: &mut Memory<H>, car: Value, cdr: Value, tag: Tag) -> Result<(), Error> {
        let rib = memory.allocate(car, cdr.set_tag(tag))?;
        memory.push(rib.into())?;
        Ok(())
    }

    #[inline(always)]
    fn set_field<H: Heap>(
        memory: &mut Memory<H>,
        set_field: fn(&mut Memory<H>, Value, Value) -> Result<(), stak_vm::Error>,
    ) -> Result<(), Error> {
        let [x, y] = memory.pop_many()?;

        set_field(memory, x, y)?;
        memory.push(y)?;

        Ok(())
    }

    fn tag<H: Heap>(
        memory: &mut Memory<H>,
        field: impl Fn(&Memory<H>, Value) -> Result<Value, stak_vm::Error>,
    ) -> Result<(), Error> {
        memory.operate_top(|memory, value| {
            Ok(if let Some(cons) = field(memory, value)?.to_cons() {
                Number::from_i64(cons.tag() as _)
            } else {
                Default::default()
            }
            .into())
        })?;

        Ok(())
    }

    fn make_bytevector<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [length, fill] = memory.pop_many()?;
        let length = Self::nonnegative_integer(Number::try_from(length)?)?;
        let fill = Self::byte(Number::try_from(fill)?)?;
        let root = if length == 0 {
            memory.null()?
        } else {
            let root = build_filled_vector_node(memory, 0, length, vector_height(length), fill)?;
            let bytevector = memory.allocate(
                Number::from_i64(length as _).into(),
                root.set_tag(BYTEVECTOR_TAG).into(),
            )?;
            let _ = memory.pop()?;
            memory.push(bytevector.into())?;
            return Ok(());
        };

        let bytevector = memory.allocate(
            Number::from_i64(length as _).into(),
            root.set_tag(BYTEVECTOR_TAG).into(),
        )?;
        memory.push(bytevector.into())?;

        Ok(())
    }

    fn make_string<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [length, fill] = memory.pop_many()?;
        let length = Self::nonnegative_integer(Number::try_from(length)?)?;
        let fill = Number::try_from(fill)?;
        code_point(fill)?;

        memory.push(memory.null()?.into())?;
        for _ in 0..length {
            let code_points = memory
                .top()?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            let code_points = memory.cons(fill.into(), code_points)?;
            memory.set_top(code_points.into())?;
        }

        let code_points = memory
            .top()?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let string = memory.allocate(
            Number::from_i64(length as _).into(),
            code_points.set_tag(STRING_TAG).into(),
        )?;
        memory.set_top(string.into())?;
        Ok(())
    }

    fn nonnegative_integer(number: Number) -> Result<usize, Error> {
        let float = number.to_f64();
        let integer = number.to_i64();
        if float != integer as f64 || integer < 0 {
            return Err(stak_vm::Error::NumberExpected.into());
        }

        Ok(usize::try_from(integer as u64).map_err(|_| stak_vm::Error::NumberExpected)?)
    }

    fn byte(number: Number) -> Result<u8, Error> {
        let number = Self::nonnegative_integer(number)?;
        Ok(u8::try_from(number).map_err(|_| stak_vm::Error::NumberExpected)?)
    }

    fn bytevector_copy<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [
            destination,
            destination_offset,
            source,
            source_start,
            source_end,
        ] = memory.pop_many()?;
        let destination_offset = Self::nonnegative_integer(Number::try_from(destination_offset)?)?;
        let source_start = Self::nonnegative_integer(Number::try_from(source_start)?)?;
        let source_end = Self::nonnegative_integer(Number::try_from(source_end)?)?;
        let (destination_root, destination_length) = bytevector_info(memory, destination)?;
        let (source_root, source_length) = bytevector_info(memory, source)?;

        if source_start > source_end
            || source_end > source_length
            || destination_offset > destination_length
            || source_end - source_start > destination_length - destination_offset
        {
            return Err(stak_vm::Error::InvalidMemoryAccess.into());
        }

        let count = source_end - source_start;
        let overlaps_to_the_right = destination_root == source_root
            && destination_offset > source_start
            && destination_offset < source_end;

        if overlaps_to_the_right {
            for offset in (0..count).rev() {
                let byte =
                    bytevector_get(memory, source_root, source_length, source_start + offset)?;
                bytevector_set(
                    memory,
                    destination_root,
                    destination_length,
                    destination_offset + offset,
                    byte,
                )?;
            }
        } else {
            let source_root = source_root.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
            let destination_root = destination_root
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            let mut source = (count > 0)
                .then(|| {
                    ListVectorCursor::<VECTOR_FACTOR>::new(
                        memory,
                        source_root,
                        source_length,
                        source_start,
                    )
                })
                .transpose()?;
            let mut destination = (count > 0)
                .then(|| {
                    ListVectorCursor::<VECTOR_FACTOR>::new(
                        memory,
                        destination_root,
                        destination_length,
                        destination_offset,
                    )
                })
                .transpose()?;

            for _ in 0..count {
                let source = source.as_mut().unwrap().next(memory)?;
                let byte = Number::try_from(memory.car(source)?)?.to_i64();
                let byte = u8::try_from(byte).map_err(|_| stak_vm::Error::NumberExpected)?;
                let destination = destination.as_mut().unwrap().next(memory)?;
                memory.set_car(destination, Number::from_i64(i64::from(byte)).into())?;
            }
        }

        memory.push(memory.boolean(false)?.into())?;
        Ok(())
    }

    fn utf8_encoded_length<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [code_points] = memory.pop_many()?;
        let mut code_points = code_points.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
        let null = memory.null()?;
        let mut length = 0usize;

        while code_points != null {
            let code_point = Number::try_from(memory.car(code_points)?)?;
            length = length
                .checked_add(encode_utf8_code_point(code_point, &mut [0; 4])?)
                .ok_or(stak_vm::Error::NumberExpected)?;
            code_points = memory
                .cdr(code_points)?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
        }

        let length = i64::try_from(length).map_err(|_| stak_vm::Error::NumberExpected)?;
        memory.push(Number::from_i64(length).into())?;
        Ok(())
    }

    fn copy_code_points<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [source, maximum] = memory.pop_many()?;
        let maximum = Self::nonnegative_integer(Number::try_from(maximum)?)?;

        memory.push(source.to_cons().ok_or(stak_vm::Error::ConsExpected)?.into())?;
        memory.push(memory.null()?.into())?;

        let mut length = 0;
        while length < maximum {
            let source_slot = memory.tail(memory.stack(), 1)?;
            let source = memory
                .car(source_slot)?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            if source == memory.null()? {
                break;
            }

            let code_point = Number::try_from(memory.car(source)?)?;
            let next = memory
                .cdr(source)?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            memory.set_car(source_slot, next.into())?;

            let copied = memory
                .top()?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            let copied = memory.cons(code_point.into(), copied)?;
            memory.set_top(copied.into())?;
            length += 1;
        }

        let mut current = memory
            .top()?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let tail = current;
        let null = memory.null()?;
        let mut head = null;
        while current != null {
            let next = memory
                .cdr(current)?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
            memory.set_cdr(current, head.set_tag(Type::Pair as _).into())?;
            head = current;
            current = next;
        }
        memory.set_top(head.into())?;
        memory.push(tail.into())?;

        let head_slot = memory.tail(memory.stack(), 1)?;
        let head = memory
            .car(head_slot)?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let string = memory.allocate(
            Number::from_i64(length as _).into(),
            head.set_tag(STRING_TAG).into(),
        )?;
        let head_slot = memory.tail(memory.stack(), 1)?;
        memory.set_car(head_slot, string.into())?;

        let tail = memory
            .top()?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let source_slot = memory.tail(memory.stack(), 2)?;
        let source = memory
            .car(source_slot)?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let metadata = memory.cons(tail.into(), source)?;
        let string_slot = memory.tail(memory.stack(), 1)?;
        let string = memory.car(string_slot)?;
        let result = memory.cons(string, metadata)?;

        for _ in 0..3 {
            memory.pop()?;
        }
        memory.push(result.into())?;
        Ok(())
    }

    fn utf8_encode<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [code_points, destination, start, end] = memory.pop_many()?;
        let mut code_points = code_points.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
        let start = Self::nonnegative_integer(Number::try_from(start)?)?;
        let end = Self::nonnegative_integer(Number::try_from(end)?)?;
        let (root, length) = bytevector_info(memory, destination)?;
        if start > end || end > length {
            return Err(stak_vm::Error::InvalidMemoryAccess.into());
        }

        let null = memory.null()?;
        let mut written = 0usize;
        let mut cursor = (start < end)
            .then(|| {
                ListVectorCursor::<VECTOR_FACTOR>::new(
                    memory,
                    root.to_cons().ok_or(stak_vm::Error::ConsExpected)?,
                    length,
                    start,
                )
            })
            .transpose()?;

        while code_points != null {
            let mut bytes = [0; 4];
            let byte_count =
                encode_utf8_code_point(Number::try_from(memory.car(code_points)?)?, &mut bytes)?;
            if byte_count > end - start - written {
                break;
            }

            for &byte in &bytes[..byte_count] {
                let cell = cursor.as_mut().unwrap().next(memory)?;
                memory.set_car(cell, Number::from_i64(i64::from(byte)).into())?;
            }
            written += byte_count;
            code_points = memory
                .cdr(code_points)?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
        }

        let written = i64::try_from(written).map_err(|_| stak_vm::Error::NumberExpected)?;
        let result = memory.allocate(code_points.into(), Number::from_i64(written).into())?;
        memory.push(result.into())?;
        Ok(())
    }

    fn utf8_decode<H: Heap>(memory: &mut Memory<H>) -> Result<(), Error> {
        let [source, start, end, maximum, final_input, strict] = memory.pop_many()?;
        let start = Self::nonnegative_integer(Number::try_from(start)?)?;
        let end = Self::nonnegative_integer(Number::try_from(end)?)?;
        let maximum = Self::nonnegative_integer(Number::try_from(maximum)?)?;
        let (root, length) = bytevector_info(memory, source)?;
        if start > end || end > length {
            return Err(stak_vm::Error::InvalidMemoryAccess.into());
        }

        let final_input = final_input != memory.boolean(false)?.into();
        let strict = strict != memory.boolean(false)?.into();
        let input_length = (end - start).min(UTF8_BUFFER_SIZE);
        let mut bytes = [0; UTF8_BUFFER_SIZE];
        if input_length > 0 {
            let root = root.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
            let mut cursor = ListVectorCursor::<VECTOR_FACTOR>::new(memory, root, length, start)?;
            for byte in &mut bytes[..input_length] {
                let cell = cursor.next(memory)?;
                let value = Number::try_from(memory.car(cell)?)?.to_i64();
                *byte = u8::try_from(value).map_err(|_| stak_vm::Error::NumberExpected)?;
            }
        }

        let mut code_points = [0; UTF8_BUFFER_SIZE];
        let mut code_point_count = 0;
        let mut consumed = 0;
        let mut invalid = false;

        while consumed < input_length && code_point_count < maximum {
            let byte = bytes[consumed];
            let sequence_length = utf8_sequence_length(byte);
            if sequence_length == 1 {
                code_points[code_point_count] = u32::from(byte);
                code_point_count += 1;
                consumed += 1;
                continue;
            }

            let available = input_length - consumed;
            let prefix = &bytes[consumed..input_length.min(consumed + sequence_length)];
            let has_invalid_continuation = sequence_length > 1
                && prefix[1..]
                    .iter()
                    .any(|&byte| !(0x80..0xc0).contains(&byte));
            let code_point = (sequence_length > 1 && available >= sequence_length)
                .then(|| utf8_decode_sequence(&bytes[consumed..consumed + sequence_length]))
                .flatten();

            if code_point.is_none()
                && sequence_length > 1
                && available < sequence_length
                && !final_input
                && !has_invalid_continuation
            {
                break;
            }

            if let Some(code_point) = code_point {
                code_points[code_point_count] = code_point;
                code_point_count += 1;
                consumed += sequence_length;
            } else if strict {
                invalid = true;
                break;
            } else {
                code_points[code_point_count] = UTF8_REPLACEMENT_CODE_POINT;
                code_point_count += 1;
                consumed += 1;
            }
        }

        if invalid {
            memory.push(memory.boolean(false)?.into())?;
            return Ok(());
        }

        let mut list = memory.null()?;
        let mut has_code_points = false;
        for &code_point in code_points[..code_point_count].iter().rev() {
            list = memory.cons(Number::from_i64(i64::from(code_point)).into(), list)?;
            if !has_code_points {
                memory.push(list.into())?;
                list = memory
                    .top()?
                    .to_cons()
                    .ok_or(stak_vm::Error::ConsExpected)?;
                has_code_points = true;
            }
        }
        if !has_code_points {
            memory.push(list.into())?;
            list = memory
                .top()?
                .to_cons()
                .ok_or(stak_vm::Error::ConsExpected)?;
        }

        let tail = memory
            .top()?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let endpoints = memory.cons(list.into(), tail)?;
        memory.set_top(endpoints.into())?;

        let decoded =
            i64::try_from(code_point_count).map_err(|_| stak_vm::Error::NumberExpected)?;
        let consumed = i64::try_from(consumed).map_err(|_| stak_vm::Error::NumberExpected)?;
        let counts = memory.allocate(
            Number::from_i64(decoded).into(),
            Number::from_i64(consumed).into(),
        )?;
        let endpoints = memory
            .top()?
            .to_cons()
            .ok_or(stak_vm::Error::ConsExpected)?;
        let result = memory.cons(endpoints.into(), counts)?;
        memory.pop()?;
        memory.push(result.into())?;
        Ok(())
    }
}

fn encode_utf8_code_point(number: Number, bytes: &mut [u8; 4]) -> Result<usize, Error> {
    let code_point = code_point(number)?;
    let character = char::from_u32(code_point).ok_or(stak_vm::Error::NumberExpected)?;
    Ok(character.encode_utf8(bytes).len())
}

fn code_point(number: Number) -> Result<u32, Error> {
    let float = number.to_f64();
    let integer = number.to_i64();
    if integer < 0 || float != integer as f64 {
        return Err(stak_vm::Error::NumberExpected.into());
    }
    let code_point = u32::try_from(integer).map_err(|_| stak_vm::Error::NumberExpected)?;
    if code_point > 0x10ffff || (0xd800..=0xdfff).contains(&code_point) {
        return Err(stak_vm::Error::NumberExpected.into());
    }

    Ok(code_point)
}

const fn utf8_sequence_length(byte: u8) -> usize {
    match byte {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

fn utf8_decode_sequence(bytes: &[u8]) -> Option<u32> {
    let continuation = |byte| (0x80..0xc0).contains(&byte);

    match *bytes {
        [byte] if byte < 0x80 => Some(u32::from(byte)),
        [first, second] if (0xc2..0xe0).contains(&first) && continuation(second) => {
            Some((u32::from(first & 0x1f) << 6) | u32::from(second & 0x3f))
        }
        [first, second, third]
            if (0xe0..0xf0).contains(&first)
                && continuation(second)
                && continuation(third)
                && (first != 0xe0 || second >= 0xa0)
                && (first != 0xed || second < 0xa0) =>
        {
            Some(
                (u32::from(first & 0x0f) << 12)
                    | (u32::from(second & 0x3f) << 6)
                    | u32::from(third & 0x3f),
            )
        }
        [first, second, third, fourth]
            if (0xf0..0xf5).contains(&first)
                && continuation(second)
                && continuation(third)
                && continuation(fourth)
                && (first != 0xf0 || second >= 0x90)
                && (first != 0xf4 || second < 0x90) =>
        {
            Some(
                (u32::from(first & 0x07) << 18)
                    | (u32::from(second & 0x3f) << 12)
                    | (u32::from(third & 0x3f) << 6)
                    | u32::from(fourth & 0x3f),
            )
        }
        _ => None,
    }
}

fn bytevector_info<H: Heap>(memory: &Memory<H>, value: Value) -> Result<(Value, usize), Error> {
    let bytevector = value.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
    let root = memory.cdr(bytevector)?;
    if root.tag() != BYTEVECTOR_TAG {
        return Err(stak_vm::Error::ConsExpected.into());
    }

    let length = bytevector_length(memory, bytevector)?;
    Ok((root, length))
}

fn bytevector_length<H: Heap>(memory: &Memory<H>, bytevector: Cons) -> Result<usize, Error> {
    let number = Number::try_from(memory.car(bytevector)?)?;
    let float = number.to_f64();
    let integer = number.to_i64();
    if integer < 0 || float != integer as f64 {
        return Err(stak_vm::Error::NumberExpected.into());
    }

    usize::try_from(integer as u64).map_err(|_| stak_vm::Error::NumberExpected.into())
}

fn bytevector_get<H: Heap>(
    memory: &Memory<H>,
    root: Value,
    length: usize,
    index: usize,
) -> Result<u8, Error> {
    let root = root.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
    let cell = list_vector_cell::<VECTOR_FACTOR, _>(memory, root, length, index)?;
    let value = Number::try_from(memory.car(cell)?)?.to_i64();
    u8::try_from(value).map_err(|_| stak_vm::Error::NumberExpected.into())
}

fn bytevector_set<H: Heap>(
    memory: &mut Memory<H>,
    root: Value,
    length: usize,
    index: usize,
    byte: u8,
) -> Result<(), Error> {
    let root = root.to_cons().ok_or(stak_vm::Error::ConsExpected)?;
    let cell = list_vector_cell::<VECTOR_FACTOR, _>(memory, root, length, index)?;
    memory.set_car(cell, Number::from_i64(i64::from(byte)).into())?;
    Ok(())
}

fn vector_height(mut length: usize) -> usize {
    let mut height = 0;

    while length > VECTOR_FACTOR {
        length = 1 + (length - 1) / VECTOR_FACTOR;
        height += 1;
    }

    height
}

fn vector_capacity(height: usize) -> usize {
    (0..height).fold(1, |capacity, _| capacity.saturating_mul(VECTOR_FACTOR))
}

fn build_filled_vector_node<H: Heap>(
    memory: &mut Memory<H>,
    start: usize,
    end: usize,
    height: usize,
    fill: u8,
) -> Result<stak_vm::Cons, Error> {
    if height == 0 {
        let mut list = memory.null()?;
        let fill = Number::from_i64(i64::from(fill)).into();

        for _ in start..end {
            list = memory.cons(fill, list)?;
        }

        memory.push(list.into())?;
        return Ok(memory.top()?.assume_cons());
    }

    let capacity = vector_capacity(height);
    let mut cursor = start;
    let mut children = 0;

    while cursor < end {
        let child_end = cursor.saturating_add(capacity).min(end);
        build_filled_vector_node(memory, cursor, child_end, height - 1, fill)?;
        children += 1;
        cursor = child_end;
    }

    let mut list = memory.null()?;
    for _ in 0..children {
        let child = memory.pop()?;
        list = memory.cons(child, list)?;
    }

    memory.push(list.into())?;
    Ok(memory.top()?.assume_cons())
}

impl<H: Heap, D: Device, F: FileSystem, P: ProcessContext, C: Clock> PrimitiveSet<H>
    for SmallPrimitiveSet<D, F, P, C>
{
    type Error = Error;

    #[maybe_async]
    fn operate(&mut self, memory: &mut Memory<H>, primitive: usize) -> Result<(), Self::Error> {
        match primitive {
            Primitive::RIB => {
                let [car, cdr, tag] = memory.pop_many()?;

                Self::rib(memory, car, cdr, tag.assume_number().to_i64() as _)?;
            }
            Primitive::CLOSE => {
                let closure = memory.pop()?;

                Self::rib(
                    memory,
                    memory.car_value(closure)?,
                    memory.stack().into(),
                    Type::Procedure as _,
                )?;
            }
            Primitive::UNBIND => {
                let [_, x] = memory.pop_many()?;
                memory.push(x)?;
            }
            Primitive::IS_RIB => {
                memory.operate_top(|memory, value| Ok(memory.boolean(value.is_cons())?.into()))?
            }
            Primitive::CAR => memory.operate_top(Memory::car_value)?,
            Primitive::CDR => memory.operate_top(Memory::cdr_value)?,
            Primitive::TAG => Self::tag(memory, Memory::cdr_value)?,
            Primitive::SET_CAR => Self::set_field(memory, Memory::set_car_value)?,
            Primitive::SET_CDR => Self::set_field(memory, Memory::set_cdr_value)?,
            Primitive::EQUAL => {
                let [x, y] = memory.pop_many()?;
                memory.push(memory.boolean(x == y)?.into())?;
            }
            Primitive::LESS_THAN => Self::operate_comparison(memory, |x, y| x < y)?,
            Primitive::ADD => memory.operate_binary(Add::add)?,
            Primitive::SUBTRACT => memory.operate_binary(Sub::sub)?,
            Primitive::MULTIPLY => memory.operate_binary(Mul::mul)?,
            Primitive::DIVIDE => memory.try_operate_binary(Number::divide)?,
            Primitive::REMAINDER => memory.try_operate_binary(Number::remainder)?,
            Primitive::EXPT => memory.operate_binary(Number::power)?,
            Primitive::HALT => return Err(Error::Halt),
            Primitive::MAKE_BYTEVECTOR => Self::make_bytevector(memory)?,
            Primitive::BYTEVECTOR_COPY => Self::bytevector_copy(memory)?,
            Primitive::UTF8_ENCODED_LENGTH => Self::utf8_encoded_length(memory)?,
            Primitive::UTF8_ENCODE => Self::utf8_encode(memory)?,
            Primitive::UTF8_DECODE => Self::utf8_decode(memory)?,
            Primitive::COPY_CODE_POINTS => Self::copy_code_points(memory)?,
            Primitive::MAKE_STRING => Self::make_string(memory)?,
            Primitive::NULL | Primitive::PAIR => {
                maybe_await!(self.type_check.operate(memory, primitive - Primitive::NULL))?
            }
            Primitive::ASSQ
            | Primitive::CONS
            | Primitive::MEMQ
            | Primitive::TAIL
            | Primitive::LENGTH => {
                maybe_await!(self.list.operate(memory, primitive - Primitive::ASSQ))?
            }
            Primitive::EQV | Primitive::EQUAL_INNER => {
                maybe_await!(self.equal.operate(memory, primitive - Primitive::EQV))?
            }
            Primitive::QUOTIENT => maybe_await!(
                self.arithmetic
                    .operate(memory, primitive - Primitive::QUOTIENT)
            )?,
            Primitive::READ | Primitive::WRITE | Primitive::WRITE_ERROR => {
                maybe_await!(self.device.operate(memory, primitive - Primitive::READ))?
            }
            Primitive::OPEN_FILE
            | Primitive::CLOSE_FILE
            | Primitive::READ_FILE
            | Primitive::WRITE_FILE
            | Primitive::DELETE_FILE
            | Primitive::EXISTS_FILE
            | Primitive::FLUSH_FILE
            | Primitive::READ_FILE_BULK
            | Primitive::WRITE_FILE_BULK => {
                maybe_await!(self.file.operate(memory, primitive - Primitive::OPEN_FILE))?
            }
            Primitive::COMMAND_LINE | Primitive::ENVIRONMENT_VARIABLES => maybe_await!(
                self.process_context
                    .operate(memory, primitive - Primitive::COMMAND_LINE)
            )?,
            Primitive::CURRENT_JIFFY | Primitive::JIFFIES_PER_SECOND => maybe_await!(
                self.time
                    .operate(memory, primitive - Primitive::CURRENT_JIFFY)
            )?,
            Primitive::EXPONENTIATION
            | Primitive::LOGARITHM
            | Primitive::INFINITE
            | Primitive::NAN
            | Primitive::SQRT
            | Primitive::COS
            | Primitive::SIN
            | Primitive::TAN
            | Primitive::ACOS
            | Primitive::ASIN
            | Primitive::ATAN => maybe_await!(
                self.inexact
                    .operate(memory, primitive - Primitive::EXPONENTIATION)
            )?,
            _ => return Err(stak_vm::Error::IllegalPrimitive.into()),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stak_device::VoidDevice;
    use stak_file::VoidFileSystem;
    use stak_process_context::VoidProcessContext;
    use stak_time::VoidClock;
    use stak_util::block_on;

    type TestPrimitiveSet =
        SmallPrimitiveSet<VoidDevice, VoidFileSystem, VoidProcessContext, VoidClock>;

    fn primitive_set() -> TestPrimitiveSet {
        SmallPrimitiveSet::new(
            VoidDevice::new(),
            VoidFileSystem::new(),
            VoidProcessContext::new(),
            VoidClock::new(),
        )
    }

    fn make_bytevector(memory: &mut Memory<[Value; 50000]>, length: usize, fill: u8) -> Value {
        memory.push(Number::from_i64(length as _).into()).unwrap();
        memory
            .push(Number::from_i64(i64::from(fill)).into())
            .unwrap();
        block_on!(primitive_set().operate(memory, Primitive::MAKE_BYTEVECTOR)).unwrap();
        memory.pop().unwrap()
    }

    fn stack_value(memory: &Memory<[Value; 50000]>, index: usize) -> Value {
        let cell = memory.tail(memory.stack(), index).unwrap();
        memory.car(cell).unwrap()
    }

    fn copy_bytevector(
        memory: &mut Memory<[Value; 50000]>,
        destination_index: usize,
        destination_offset: usize,
        source_index: usize,
        source_start: usize,
        source_end: usize,
    ) -> Result<(), Error> {
        let destination = stack_value(memory, destination_index);
        memory.push(destination).unwrap();
        memory
            .push(Number::from_i64(destination_offset as _).into())
            .unwrap();
        let source = stack_value(memory, source_index + 2);
        memory.push(source).unwrap();
        memory
            .push(Number::from_i64(source_start as _).into())
            .unwrap();
        memory
            .push(Number::from_i64(source_end as _).into())
            .unwrap();

        block_on!(primitive_set().operate(memory, Primitive::BYTEVECTOR_COPY))?;
        memory.pop().unwrap();
        Ok(())
    }

    fn decode_bytevector(
        memory: &mut Memory<[Value; 50000]>,
        source_index: usize,
        start: usize,
        end: usize,
        maximum: usize,
        final_input: bool,
        strict: bool,
    ) -> Value {
        let source = stack_value(memory, source_index);
        memory.push(source).unwrap();
        memory.push(Number::from_i64(start as _).into()).unwrap();
        memory.push(Number::from_i64(end as _).into()).unwrap();
        memory.push(Number::from_i64(maximum as _).into()).unwrap();
        let final_input = memory.boolean(final_input).unwrap();
        memory.push(final_input.into()).unwrap();
        let strict = memory.boolean(strict).unwrap();
        memory.push(strict.into()).unwrap();
        block_on!(primitive_set().operate(memory, Primitive::UTF8_DECODE)).unwrap();
        memory.pop().unwrap()
    }

    #[test]
    fn bytevector_copy_crosses_vector_boundaries() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let source = make_bytevector(&mut memory, 4097, 0);
        memory.push(source).unwrap();
        let destination = make_bytevector(&mut memory, 4097, 255);
        memory.push(destination).unwrap();
        let source = stack_value(&memory, 1);

        let source_root = bytevector_info(&memory, source).unwrap().0;
        for (index, byte) in [(0, 11), (63, 22), (64, 33), (4095, 44), (4096, 55)] {
            bytevector_set(&mut memory, source_root, 4097, index, byte).unwrap();
        }

        copy_bytevector(&mut memory, 0, 0, 1, 0, 4097).unwrap();
        let destination = stack_value(&memory, 0);
        let (destination_root, destination_length) = bytevector_info(&memory, destination).unwrap();
        for (index, expected) in [(0, 11), (63, 22), (64, 33), (4095, 44), (4096, 55)] {
            assert_eq!(
                bytevector_get(&memory, destination_root, destination_length, index).unwrap(),
                expected
            );
        }
        for index in 0..destination_length {
            if !matches!(index, 0 | 63 | 64 | 4095 | 4096) {
                assert_eq!(
                    bytevector_get(&memory, destination_root, destination_length, index).unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn bytevector_copy_handles_overlap_in_both_directions() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let bytevector = make_bytevector(&mut memory, 4, 0);
        memory.push(bytevector).unwrap();
        let bytevector = stack_value(&memory, 0);
        let root = bytevector_info(&memory, bytevector).unwrap().0;

        for (index, byte) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
            bytevector_set(&mut memory, root, 4, index, byte).unwrap();
        }
        copy_bytevector(&mut memory, 0, 1, 0, 0, 3).unwrap();
        let bytevector = stack_value(&memory, 0);
        let root = bytevector_info(&memory, bytevector).unwrap().0;
        assert_eq!(bytevector_get(&memory, root, 4, 0).unwrap(), 0);
        assert_eq!(bytevector_get(&memory, root, 4, 1).unwrap(), 0);
        assert_eq!(bytevector_get(&memory, root, 4, 2).unwrap(), 1);
        assert_eq!(bytevector_get(&memory, root, 4, 3).unwrap(), 2);

        let root = bytevector_info(&memory, bytevector).unwrap().0;
        for (index, byte) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
            bytevector_set(&mut memory, root, 4, index, byte).unwrap();
        }
        copy_bytevector(&mut memory, 0, 0, 0, 1, 4).unwrap();
        let bytevector = stack_value(&memory, 0);
        let root = bytevector_info(&memory, bytevector).unwrap().0;
        assert_eq!(bytevector_get(&memory, root, 4, 0).unwrap(), 1);
        assert_eq!(bytevector_get(&memory, root, 4, 1).unwrap(), 2);
        assert_eq!(bytevector_get(&memory, root, 4, 2).unwrap(), 3);
        assert_eq!(bytevector_get(&memory, root, 4, 3).unwrap(), 3);
    }

    #[test]
    fn bytevector_copy_rejects_invalid_ranges() {
        for (destination_offset, source_start, source_end) in [(0, 2, 1), (0, 0, 4), (3, 0, 2)] {
            let mut memory = Memory::new([Default::default(); 50000]).unwrap();
            let source = make_bytevector(&mut memory, 3, 65);
            memory.push(source).unwrap();
            let destination = make_bytevector(&mut memory, 4, 0);
            memory.push(destination).unwrap();

            assert_eq!(
                copy_bytevector(
                    &mut memory,
                    0,
                    destination_offset,
                    1,
                    source_start,
                    source_end,
                ),
                Err(Error::Vm(stak_vm::Error::InvalidMemoryAccess))
            );
        }
    }

    #[test]
    fn bytevector_copy_accepts_empty_ranges_and_empty_bytevectors() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let source = make_bytevector(&mut memory, 0, 0);
        memory.push(source).unwrap();
        let destination = make_bytevector(&mut memory, 0, 0);
        memory.push(destination).unwrap();

        copy_bytevector(&mut memory, 0, 0, 1, 0, 0).unwrap();
        let destination = stack_value(&memory, 0);
        assert_eq!(bytevector_info(&memory, destination).unwrap().1, 0);
    }

    #[test]
    fn utf8_codec_handles_multibyte_sequences() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let mut code_points = memory.null().unwrap();
        for code_point in [65, 233, 12354].into_iter().rev() {
            code_points = memory
                .cons(Number::from_i64(code_point).into(), code_points)
                .unwrap();
        }
        memory.push(code_points.into()).unwrap();

        let destination = make_bytevector(&mut memory, 6, 0);
        memory.push(destination).unwrap();
        let code_points = stack_value(&memory, 1);
        memory.push(code_points).unwrap();
        let destination = stack_value(&memory, 1);
        memory.push(destination).unwrap();
        memory.push(Number::from_i64(0).into()).unwrap();
        memory.push(Number::from_i64(6).into()).unwrap();
        block_on!(primitive_set().operate(&mut memory, Primitive::UTF8_ENCODE)).unwrap();

        let result = memory.pop().unwrap();
        assert_eq!(
            memory.car_value(result).unwrap(),
            memory.null().unwrap().into()
        );
        assert_eq!(
            Number::try_from(memory.cdr_value(result).unwrap())
                .unwrap()
                .to_i64(),
            6
        );
        let destination = stack_value(&memory, 0);
        let (root, length) = bytevector_info(&memory, destination).unwrap();
        for (index, expected) in [65, 195, 169, 227, 129, 130].into_iter().enumerate() {
            assert_eq!(
                bytevector_get(&memory, root, length, index).unwrap(),
                expected
            );
        }

        let source = stack_value(&memory, 0);
        memory.push(source).unwrap();
        memory.push(Number::from_i64(0).into()).unwrap();
        memory.push(Number::from_i64(6).into()).unwrap();
        memory.push(Number::from_i64(64).into()).unwrap();
        let true_value = memory.boolean(true).unwrap();
        memory.push(true_value.into()).unwrap();
        let true_value = memory.boolean(true).unwrap();
        memory.push(true_value.into()).unwrap();
        block_on!(primitive_set().operate(&mut memory, Primitive::UTF8_DECODE)).unwrap();

        let result = memory.pop().unwrap();
        let endpoints = memory.car_value(result).unwrap();
        let counts = memory.cdr_value(result).unwrap();
        assert_eq!(
            Number::try_from(memory.cdr_value(counts).unwrap())
                .unwrap()
                .to_i64(),
            6
        );
        assert_eq!(
            Number::try_from(memory.car_value(counts).unwrap())
                .unwrap()
                .to_i64(),
            3
        );
        let tail = memory.cdr_value(endpoints).unwrap().to_cons().unwrap();
        assert_eq!(
            Number::try_from(memory.car(tail).unwrap())
                .unwrap()
                .to_i64(),
            12354
        );
        assert_eq!(memory.cdr(tail).unwrap(), memory.null().unwrap().into());
        let mut list = memory.car_value(endpoints).unwrap().to_cons().unwrap();
        for expected in [65, 233, 12354] {
            assert_eq!(
                Number::try_from(memory.car(list).unwrap())
                    .unwrap()
                    .to_i64(),
                expected
            );
            list = memory.cdr(list).unwrap().to_cons().unwrap();
        }
        assert_eq!(list, memory.null().unwrap());
    }

    #[test]
    fn utf8_decoder_rejects_malformed_sequences_without_panicking() {
        for bytes in [
            &[192, 128][..],
            &[128],
            &[226, 128],
            &[237, 160, 128],
            &[244, 144, 128, 128],
            &[245, 128, 128, 128],
        ] {
            let mut memory = Memory::new([Default::default(); 50000]).unwrap();
            let source = make_bytevector(&mut memory, bytes.len(), 0);
            memory.push(source).unwrap();
            let source = stack_value(&memory, 0);
            let (root, length) = bytevector_info(&memory, source).unwrap();
            for (index, &byte) in bytes.iter().enumerate() {
                bytevector_set(&mut memory, root, length, index, byte).unwrap();
            }

            let result = decode_bytevector(&mut memory, 0, 0, bytes.len(), 64, true, true);
            assert_eq!(result, memory.boolean(false).unwrap().into());
        }
    }

    #[test]
    fn utf8_decoder_defers_an_incomplete_nonfinal_sequence() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let source = make_bytevector(&mut memory, 3, 0);
        memory.push(source).unwrap();
        let source = stack_value(&memory, 0);
        let (root, length) = bytevector_info(&memory, source).unwrap();
        for (index, byte) in [227, 129, 130].into_iter().enumerate() {
            bytevector_set(&mut memory, root, length, index, byte).unwrap();
        }

        let result = decode_bytevector(&mut memory, 0, 0, 2, 64, false, true);
        let endpoints = memory.car_value(result).unwrap();
        let counts = memory.cdr_value(result).unwrap();
        assert_eq!(
            memory.car_value(endpoints).unwrap(),
            memory.null().unwrap().into()
        );
        assert_eq!(
            Number::try_from(memory.cdr_value(counts).unwrap())
                .unwrap()
                .to_i64(),
            0
        );

        let result = decode_bytevector(&mut memory, 0, 0, 3, 64, true, true);
        let endpoints = memory.car_value(result).unwrap();
        let counts = memory.cdr_value(result).unwrap();
        assert_eq!(
            Number::try_from(memory.cdr_value(counts).unwrap())
                .unwrap()
                .to_i64(),
            3
        );
        let code_points = memory.car_value(endpoints).unwrap().to_cons().unwrap();
        assert_eq!(
            Number::try_from(memory.car(code_points).unwrap())
                .unwrap()
                .to_i64(),
            12354
        );
        assert_eq!(
            memory.cdr(code_points).unwrap(),
            memory.null().unwrap().into()
        );
    }

    #[test]
    fn copy_code_points_returns_an_independent_prefix_and_remainder() {
        let mut memory = Memory::new([Default::default(); 50000]).unwrap();
        let mut source = memory.null().unwrap();
        for code_point in [65, 66, 67].into_iter().rev() {
            source = memory
                .cons(Number::from_i64(code_point).into(), source)
                .unwrap();
        }
        memory.push(source.into()).unwrap();

        let source = stack_value(&memory, 0);
        memory.push(source).unwrap();
        memory.push(Number::from_i64(2).into()).unwrap();
        block_on!(primitive_set().operate(&mut memory, Primitive::COPY_CODE_POINTS)).unwrap();

        let result = memory.pop().unwrap();
        let string = memory.car_value(result).unwrap();
        assert_eq!(
            Number::try_from(memory.car_value(string).unwrap())
                .unwrap()
                .to_i64(),
            2
        );
        assert_eq!(memory.cdr_value(string).unwrap().tag(), STRING_TAG);

        let metadata = memory.cdr_value(result).unwrap();
        let tail = memory.car_value(metadata).unwrap().to_cons().unwrap();
        let remainder = memory.cdr_value(metadata).unwrap().to_cons().unwrap();
        let copied = memory.cdr_value(string).unwrap().to_cons().unwrap();
        assert_eq!(
            Number::try_from(memory.car(copied).unwrap())
                .unwrap()
                .to_i64(),
            65
        );
        assert_eq!(
            Number::try_from(memory.car(tail).unwrap())
                .unwrap()
                .to_i64(),
            66
        );
        assert_eq!(
            Number::try_from(memory.car(remainder).unwrap())
                .unwrap()
                .to_i64(),
            67
        );

        memory.set_car(copied, Number::from_i64(90).into()).unwrap();
        let source = stack_value(&memory, 0).to_cons().unwrap();
        assert_eq!(
            Number::try_from(memory.car(source).unwrap())
                .unwrap()
                .to_i64(),
            65
        );
    }

    #[test]
    fn make_string_preserves_the_list_backed_representation() {
        let mut memory = Memory::new([Default::default(); 64]).unwrap();
        memory.push(Number::from_i64(3).into()).unwrap();
        memory.push(Number::from_i64(233).into()).unwrap();

        block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_STRING)).unwrap();

        let string = memory.pop().unwrap();
        assert_eq!(
            Number::try_from(memory.car_value(string).unwrap())
                .unwrap()
                .to_i64(),
            3
        );
        assert_eq!(memory.cdr_value(string).unwrap().tag(), STRING_TAG);

        let mut code_points = memory.cdr_value(string).unwrap().to_cons().unwrap();
        for _ in 0..3 {
            assert_eq!(
                Number::try_from(memory.car(code_points).unwrap())
                    .unwrap()
                    .to_i64(),
                233
            );
            code_points = memory.cdr(code_points).unwrap().to_cons().unwrap();
        }
        assert_eq!(code_points, memory.null().unwrap());
    }

    #[test]
    fn make_string_rejects_an_invalid_code_point() {
        let mut memory = Memory::new([Default::default(); 64]).unwrap();
        memory.push(Number::from_i64(1).into()).unwrap();
        memory.push(Number::from_i64(0x110000).into()).unwrap();

        assert_eq!(
            block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_STRING)),
            Err(Error::Vm(stak_vm::Error::NumberExpected))
        );
    }

    #[test]
    fn make_bytevector_relocates_the_filled_root() {
        let mut memory = Memory::new([Default::default(); 24]).unwrap();
        memory.push(Number::from_i64(1).into()).unwrap();
        memory.push(Number::from_i64(37).into()).unwrap();

        block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_BYTEVECTOR)).unwrap();

        let bytevector = memory.pop().unwrap();
        assert_eq!(
            memory
                .car_value(bytevector)
                .unwrap()
                .assume_number()
                .to_i64(),
            1
        );
        assert_eq!(memory.cdr_value(bytevector).unwrap().tag(), BYTEVECTOR_TAG);
    }

    #[test]
    fn make_bytevector_rejects_nonnumeric_length() {
        let mut memory = Memory::new([Default::default(); 64]).unwrap();
        let false_value = memory.boolean(false).unwrap();
        memory.push(false_value.into()).unwrap();
        memory.push(Number::from_i64(0).into()).unwrap();

        assert_eq!(
            block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_BYTEVECTOR)),
            Err(Error::Vm(stak_vm::Error::NumberExpected))
        );
    }

    #[test]
    fn make_bytevector_rejects_nonnumeric_fill() {
        let mut memory = Memory::new([Default::default(); 64]).unwrap();
        let false_value = memory.boolean(false).unwrap();
        memory.push(false_value.into()).unwrap();
        memory.push(Number::from_i64(1).into()).unwrap();
        memory.push(memory.boolean(false).unwrap().into()).unwrap();

        assert_eq!(
            block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_BYTEVECTOR)),
            Err(Error::Vm(stak_vm::Error::NumberExpected))
        );
    }
}
