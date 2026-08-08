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
use stak_vm::{Heap, Memory, Number, PrimitiveSet, Tag, Type, Value};
use winter_maybe_async::{maybe_async, maybe_await};

const BYTEVECTOR_TAG: Tag = 8;
const VECTOR_FACTOR: usize = 64;

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
            | Primitive::FLUSH_FILE => {
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
        memory.push(Number::from_i64(1).into()).unwrap();
        memory.push(false_value.into()).unwrap();

        assert_eq!(
            block_on!(primitive_set().operate(&mut memory, Primitive::MAKE_BYTEVECTOR)),
            Err(Error::Vm(stak_vm::Error::NumberExpected))
        );
    }
}
