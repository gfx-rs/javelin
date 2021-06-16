/*! Standard Portable Intermediate Representation (SPIR-V) backend
!*/

mod helpers;
mod index;
mod instructions;
mod layout;
mod writer;

pub use spirv::Capability;

use crate::{arena::Handle, back::IndexBoundsCheckPolicy};

use spirv::Word;
use std::ops;
use thiserror::Error;

struct PhysicalLayout {
    magic_number: Word,
    version: Word,
    generator: Word,
    bound: Word,
    instruction_schema: Word,
}

#[derive(Default)]
struct LogicalLayout {
    capabilities: Vec<Word>,
    extensions: Vec<Word>,
    ext_inst_imports: Vec<Word>,
    memory_model: Vec<Word>,
    entry_points: Vec<Word>,
    execution_modes: Vec<Word>,
    debugs: Vec<Word>,
    annotations: Vec<Word>,
    declarations: Vec<Word>,
    function_declarations: Vec<Word>,
    function_definitions: Vec<Word>,
}

struct Instruction {
    op: spirv::Op,
    wc: u32,
    type_id: Option<Word>,
    result_id: Option<Word>,
    operands: Vec<Word>,
}

const BITS_PER_BYTE: crate::Bytes = 8;

#[derive(Clone, Debug, Error)]
pub enum Error {
    #[error("target SPIRV-{0}.{1} is not supported")]
    UnsupportedVersion(u8, u8),
    #[error("one of the required capabilities {0:?} is missing")]
    MissingCapabilities(Vec<Capability>),
    #[error("unimplemented {0}")]
    FeatureNotImplemented(&'static str),
    #[error("module is not validated properly: {0}")]
    Validation(&'static str),
    #[error(transparent)]
    Proc(#[from] crate::proc::ProcError),
}

#[derive(Default)]
struct IdGenerator(Word);

impl IdGenerator {
    fn next(&mut self) -> Word {
        self.0 += 1;
        self.0
    }
}

struct Block {
    label_id: Word,
    body: Vec<Instruction>,
    termination: Option<Instruction>,
}

impl Block {
    fn new(label_id: Word) -> Self {
        Block {
            label_id,
            body: Vec::new(),
            termination: None,
        }
    }
}

struct LocalVariable {
    id: Word,
    instruction: Instruction,
}

struct ResultMember {
    id: Word,
    type_id: Word,
    built_in: Option<crate::BuiltIn>,
}

struct EntryPointContext {
    argument_ids: Vec<Word>,
    results: Vec<ResultMember>,
}

#[derive(Default)]
struct Function {
    signature: Option<Instruction>,
    parameters: Vec<Instruction>,
    variables: crate::FastHashMap<Handle<crate::LocalVariable>, LocalVariable>,
    blocks: Vec<Block>,
    entry_point_context: Option<EntryPointContext>,
}

impl Function {
    fn consume(&mut self, mut block: Block, termination: Instruction) {
        block.termination = Some(termination);
        self.blocks.push(block);
    }

    fn parameter_id(&self, index: u32) -> Word {
        match self.entry_point_context {
            Some(ref context) => context.argument_ids[index as usize],
            None => self.parameters[index as usize].result_id.unwrap(),
        }
    }
}

/// A SPIR-V type constructed during code generation.
///
/// In the process of writing SPIR-V, we need to synthesize various types for
/// intermediate results and such. However, it's inconvenient to use
/// `crate::Type` or `crate::TypeInner` for these, as the IR module is immutable
/// so we can't ever create a `Handle<Type>` to refer to them. So for local use
/// in the SPIR-V writer, we have this home-grown type enum that covers only the
/// cases we need (for example, it doesn't cover structs).
#[derive(Debug, PartialEq, Hash, Eq, Copy, Clone)]
enum LocalType {
    /// A scalar, vector, or pointer to one of those.
    Value {
        /// If `None`, this represents a scalar type. If `Some`, this represents
        /// a vector type of the given size.
        vector_size: Option<crate::VectorSize>,
        kind: crate::ScalarKind,
        width: crate::Bytes,
        pointer_class: Option<spirv::StorageClass>,
    },
    /// A matrix of floating-point values.
    Matrix {
        columns: crate::VectorSize,
        rows: crate::VectorSize,
        width: crate::Bytes,
    },
    Pointer {
        base: Handle<crate::Type>,
        class: spirv::StorageClass,
    },
    Image {
        dim: crate::ImageDimension,
        arrayed: bool,
        class: crate::ImageClass,
    },
    SampledImage {
        image_type_id: Word,
    },
    Sampler,
}

#[derive(Debug, PartialEq, Hash, Eq, Copy, Clone)]
enum LookupType {
    Handle(Handle<crate::Type>),
    Local(LocalType),
}

impl From<LocalType> for LookupType {
    fn from(local: LocalType) -> Self {
        Self::Local(local)
    }
}

#[derive(Debug, PartialEq, Clone, Hash, Eq)]
struct LookupFunctionType {
    parameter_type_ids: Vec<Word>,
    return_type_id: Word,
}

#[derive(Debug)]
enum Dimension {
    Scalar,
    Vector,
    Matrix,
}

#[derive(Default)]
struct CachedExpressions {
    ids: Vec<Word>,
}
impl CachedExpressions {
    fn reset(&mut self, length: usize) {
        self.ids.clear();
        self.ids.resize(length, 0);
    }
}
impl ops::Index<Handle<crate::Expression>> for CachedExpressions {
    type Output = Word;
    fn index(&self, h: Handle<crate::Expression>) -> &Word {
        let id = &self.ids[h.index()];
        if *id == 0 {
            unreachable!("Expression {:?} is not cached!", h);
        }
        id
    }
}
impl ops::IndexMut<Handle<crate::Expression>> for CachedExpressions {
    fn index_mut(&mut self, h: Handle<crate::Expression>) -> &mut Word {
        let id = &mut self.ids[h.index()];
        if *id != 0 {
            unreachable!("Expression {:?} is already cached!", h);
        }
        id
    }
}

struct GlobalVariable {
    /// Actual ID of the variable.
    id: Word,
    /// For `StorageClass::Handle` variables, this ID is recorded in the function
    /// prelude block (and reset before every function) as `OpLoad` of the variable.
    /// It is then used for all the global ops, such as `OpImageSample`.
    handle_id: Word,
}

pub struct Writer {
    physical_layout: PhysicalLayout,
    logical_layout: LogicalLayout,
    id_gen: IdGenerator,
    capabilities: crate::FastHashSet<Capability>,
    forbidden_caps: Option<&'static [Capability]>,
    debugs: Vec<Instruction>,
    annotations: Vec<Instruction>,
    flags: WriterFlags,
    index_bounds_check_policy: IndexBoundsCheckPolicy,
    void_type: Word,
    //TODO: convert most of these into vectors, addressable by handle indices
    lookup_type: crate::FastHashMap<LookupType, Word>,
    lookup_function: crate::FastHashMap<Handle<crate::Function>, Word>,
    lookup_function_type: crate::FastHashMap<LookupFunctionType, Word>,
    lookup_function_call: crate::FastHashMap<Handle<crate::Expression>, Word>,
    constant_ids: Vec<Word>,
    cached_constants: crate::FastHashMap<(crate::ScalarValue, crate::Bytes), Word>,
    global_variables: Vec<GlobalVariable>,
    cached: CachedExpressions,
    gl450_ext_inst_id: Word,
    // Just a temporary list of SPIR-V ids
    temp_list: Vec<Word>,
}

bitflags::bitflags! {
    pub struct WriterFlags: u32 {
        /// Include debug labels for everything.
        const DEBUG = 0x1;
        /// Flip Y coordinate of `BuiltIn::Position` output.
        const ADJUST_COORDINATE_SPACE = 0x2;
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    /// (Major, Minor) target version of the SPIR-V.
    pub lang_version: (u8, u8),
    /// Configuration flags for the writer.
    pub flags: WriterFlags,
    /// Set of SPIR-V allowed capabilities, if provided.
    // Note: there is a major bug currently associated with deriving the capabilities.
    // We are calling `required_capabilities`, but the semantics of this is broken.
    pub capabilities: Option<crate::FastHashSet<Capability>>,
    /// How should the generated code handle array, vector, or matrix indices
    /// that are out of range?
    pub index_bounds_check_policy: IndexBoundsCheckPolicy,
}

impl Default for Options {
    fn default() -> Self {
        let mut flags = WriterFlags::ADJUST_COORDINATE_SPACE;
        if cfg!(debug_assertions) {
            flags |= WriterFlags::DEBUG;
        }
        Options {
            lang_version: (1, 0),
            flags,
            capabilities: None,
            index_bounds_check_policy: super::IndexBoundsCheckPolicy::default(),
        }
    }
}

pub fn write_vec(
    module: &crate::Module,
    info: &crate::valid::ModuleInfo,
    options: &Options,
) -> Result<Vec<u32>, Error> {
    let mut words = Vec::new();
    let mut w = Writer::new(options)?;
    w.write(module, info, &mut words)?;
    Ok(words)
}
