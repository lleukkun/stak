use crate::{Cons, Error, Heap, Memory};

/// Returns a cell at an index in a list-backed tree vector.
pub fn list_vector_cell<const FACTOR: usize, H: Heap>(
    memory: &Memory<H>,
    root: Cons,
    length: usize,
    index: usize,
) -> Result<Cons, Error> {
    if FACTOR <= 1 || index >= length {
        return Err(Error::InvalidMemoryAccess);
    }

    let mut height = 0;
    let mut node_length = length;
    while node_length > FACTOR {
        node_length = 1 + (node_length - 1) / FACTOR;
        height += 1;
    }

    let mut stride = 1usize;
    for _ in 0..height {
        stride = stride
            .checked_mul(FACTOR)
            .ok_or(Error::InvalidMemoryAccess)?;
    }

    let mut list = root;
    let mut remainder = index;
    let mut first = true;
    while stride > 0 {
        if !first {
            list = memory.car(list)?.to_cons().ok_or(Error::ConsExpected)?;
        }
        list = memory.tail(list, remainder / stride)?;
        remainder %= stride;
        stride /= FACTOR;
        first = false;
    }

    Ok(list)
}

/// A forward cursor over cells in a list-backed tree vector.
///
/// The cursor follows leaf-list links for adjacent elements and only descends
/// from the root when it crosses a leaf boundary.
///
/// The cursor stores heap locations directly. It is valid only while the
/// underlying memory does not allocate or collect garbage, and every call to
/// [`Self::next`] must use the same memory that was passed to [`Self::new`].
pub struct ListVectorCursor<const FACTOR: usize> {
    root: Cons,
    length: usize,
    index: usize,
    cell: Cons,
}

impl<const FACTOR: usize> ListVectorCursor<FACTOR> {
    /// Creates a cursor positioned at an index.
    pub fn new<H: Heap>(
        memory: &Memory<H>,
        root: Cons,
        length: usize,
        index: usize,
    ) -> Result<Self, Error> {
        Ok(Self {
            root,
            length,
            index,
            cell: list_vector_cell::<FACTOR, _>(memory, root, length, index)?,
        })
    }

    /// Returns the current cell and advances to the next one.
    pub fn next<H: Heap>(&mut self, memory: &Memory<H>) -> Result<Cons, Error> {
        if self.index >= self.length {
            return Err(Error::InvalidMemoryAccess);
        }

        let cell = self.cell;
        self.index += 1;

        if self.index < self.length {
            self.cell = if self.index.is_multiple_of(FACTOR) {
                list_vector_cell::<FACTOR, _>(memory, self.root, self.length, self.index)?
            } else {
                memory.cdr(cell)?.to_cons().ok_or(Error::ConsExpected)?
            };
        }

        Ok(cell)
    }
}
