use std::ops::Range;

use gfx_hal::{Backend, MemoryProperties, MemoryType, MemoryTypeId};
use gfx_hal::memory::{Properties, Requirements};

use {MemoryAllocator, MemoryError};
use block::Block;
use combined::{CombinedAllocator, CombinedBlock, Type};

/// Allocator that can choose memory type based on requirements, and keeps track of allocators
/// for all given memory types.
///
/// Allocates memory blocks from the least used memory type from those which satisfy requirements.
#[derive(Debug)]
pub struct SmartAllocator<B: Backend> {
    allocators: Vec<(MemoryType, CombinedAllocator<B>)>,
    heaps: Vec<Heap>,
}

impl<B> SmartAllocator<B>
where
    B: Backend,
{
    /// Create a new smart allocator from `MemoryProperties` given by a device.
    ///
    /// ### Parameters:
    ///
    /// - `memory_properties`: memory properties describing the memory available on a device
    /// - `arena_size`: see `ArenaAllocator`
    /// - `chunks_per_block`: see `ChunkedAllocator`
    /// - `min_chunk_size`: see `ChunkedAllocator`
    /// - `max_chunk_size`: see `ChunkedAllocator`
    pub fn new(
        memory_properties: MemoryProperties,
        arena_size: u64,
        chunks_per_block: usize,
        min_chunk_size: u64,
        max_chunk_size: u64,
    ) -> Self {
        SmartAllocator {
            allocators: memory_properties
                .memory_types
                .into_iter()
                .enumerate()
                .map(|(index, memory_type)| {
                    (
                        memory_type,
                        CombinedAllocator::new(
                            MemoryTypeId(index),
                            arena_size,
                            chunks_per_block,
                            min_chunk_size,
                            max_chunk_size,
                        ),
                    )
                })
                .collect(),
            heaps: memory_properties
                .memory_heaps
                .into_iter()
                .map(|size| Heap { size, used: 0 })
                .collect(),
        }
    }
}

impl<B> MemoryAllocator<B> for SmartAllocator<B>
where
    B: Backend,
{
    type Request = (Type, Properties);
    type Block = SmartBlock<B>;

    fn alloc(
        &mut self,
        device: &B::Device,
        (ty, prop): (Type, Properties),
        reqs: Requirements,
    ) -> Result<SmartBlock<B>, MemoryError> {
        let ref mut heaps = self.heaps;
        let allocators = self.allocators.iter_mut().enumerate();

        let mut compatible_count = 0;
        let (index, &mut (memory_type, ref mut allocator)) = allocators
            .filter(|&(index, &mut (ref memory_type, _))| {
                ((1 << index) & reqs.type_mask) == (1 << index)
                    && memory_type.properties.contains(prop)
            })
            .filter(|&(_, &mut (ref memory_type, _))| {
                compatible_count += 1;
                heaps[memory_type.heap_index].available() >= (reqs.size + reqs.alignment)
            })
            .next()
            .ok_or(MemoryError::from(if compatible_count == 0 {
                MemoryError::NoCompatibleMemoryType
            } else {
                MemoryError::OutOfMemory
            }))?;

        let block = allocator.alloc(device, ty, reqs)?;
        heaps[memory_type.heap_index].alloc(block.size());

        Ok(SmartBlock(block, index))
    }

    fn free(&mut self, device: &B::Device, block: SmartBlock<B>) {
        let SmartBlock(block, index) = block;
        self.heaps[self.allocators[index].0.heap_index].free(block.size());
        self.allocators[index].1.free(device, block);
    }

    fn is_used(&self) -> bool {
        self.allocators
            .iter()
            .any(|&(_, ref allocator)| allocator.is_used())
    }

    fn dispose(mut self, device: &B::Device) -> Result<(), Self> {
        if self.is_used() {
            Err(self)
        } else {
            for (_, allocator) in self.allocators.drain(..) {
                allocator.dispose(device).unwrap();
            }
            Ok(())
        }
    }
}

#[derive(Debug)]
struct Heap {
    size: u64,
    used: u64,
}

impl Heap {
    fn available(&self) -> u64 {
        self.size - self.used
    }

    fn alloc(&mut self, size: u64) {
        self.used += size;
    }

    fn free(&mut self, size: u64) {
        self.used -= size;
    }
}

/// Opaque type for `Block` tag used by the `SmartAllocator`.
///
/// `SmartAllocator` places this tag on the memory blocks, and then use it in
/// `free` to find the memory node the block was allocated from.
#[derive(Debug)]
pub struct SmartBlock<B: Backend>(CombinedBlock<B>, usize);

impl<B> Block<B> for SmartBlock<B>
where
    B: Backend,
{
    /// Get memory of the block.
    #[inline(always)]
    fn memory(&self) -> &B::Memory {
        // Has to be valid
        self.0.memory()
    }

    /// Get memory range of the block.
    #[inline(always)]
    fn range(&self) -> Range<u64> {
        self.0.range()
    }
}
