//! Universal shader translator.
//!
//! The central structure of the crate is [`Module`].
//!
//! To improve performance and reduce memory usage, most structures are stored
//! in an [`Arena`], and can be retrieved using the corresponding [`Handle`].
#![allow(clippy::new_without_default)]
#![deny(clippy::panic)]

mod arena;
pub mod back;
pub mod front;
pub mod proc;

pub use crate::arena::{Arena, Handle};

use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasherDefault,
    num::NonZeroU32,
};

#[cfg(feature = "deserialize")]
use serde::Deserialize;
#[cfg(feature = "serialize")]
use serde::Serialize;

/// Hash map that is faster but not resilient to DoS attacks.
pub type FastHashMap<K, T> = HashMap<K, T, BuildHasherDefault<fxhash::FxHasher>>;
/// Hash set that is faster but not resilient to DoS attacks.
pub type FastHashSet<K> = HashSet<K, BuildHasherDefault<fxhash::FxHasher>>;

/// Metadata for a given module.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct Header {
    /// Major, minor and patch version.
    ///
    /// Currently used only for the SPIR-V back end.
    pub version: (u8, u8, u8),
    /// Magic number identifying the tool that generated the shader code.
    ///
    /// Can safely be set to 0.
    pub generator: u32,
}

/// Stage of the programmable pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
#[allow(missing_docs)] // The names are self evident
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

/// Class of storage for variables.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
#[allow(missing_docs)] // The names are self evident
pub enum StorageClass {
    Constant,
    Function,
    Input,
    Output,
    Private,
    StorageBuffer,
    Uniform,
    WorkGroup,
}

/// Built-in inputs and outputs.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum BuiltIn {
    // vertex
    BaseInstance,
    BaseVertex,
    ClipDistance,
    InstanceIndex,
    Position,
    VertexIndex,
    // fragment
    PointSize,
    FragCoord,
    FrontFacing,
    SampleIndex,
    FragDepth,
    // compute
    GlobalInvocationId,
    LocalInvocationId,
    LocalInvocationIndex,
    WorkGroupId,
}

/// Number of bytes.
pub type Bytes = u8;

/// Number of components in a vector.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum VectorSize {
    /// 2D vector
    Bi = 2,
    /// 3D vector
    Tri = 3,
    /// 4D vector
    Quad = 4,
}

/// Primitive type for a scalar.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Hash, Eq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum ScalarKind {
    /// Signed integer type.
    Sint,
    /// Unsigned integer type.
    Uint,
    /// Floating point type.
    Float,
    /// Boolean type.
    Bool,
}

/// Size of an array.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum ArraySize {
    /// The array size is known at compilation.
    Static(u32),
    /// The array size can change at runtime.
    Dynamic,
}

/// Describes where a struct member is placed.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum MemberOrigin {
    /// Built-in shader variable.
    BuiltIn(BuiltIn),
    /// Offset within the struct.
    Offset(u32),
}

/// The interpolation qualifier of a binding or struct field.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum Interpolation {
    /// Indicates that linear, non-perspective, correct
    //// interpolation must be used.
    NoPerspective,
    /// Indicates that no interpolation will be performed.
    Flat,
    /// Indicates a tessellation patch.
    Patch,
    /// When used with multi-sampling rasterization, allow
    /// a single interpolation location for an entire pixel.
    Centroid,
    /// When used with multi-sampling rasterization, require
    /// per-sample interpolation.
    Sample,
}

/// Member of a user-defined structure.
// Clone is used only for error reporting and is not intended for end users
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct StructMember {
    pub name: Option<String>,
    pub origin: MemberOrigin,
    pub ty: Handle<Type>,
    pub interpolation: Option<Interpolation>,
}

/// The number of dimensions an image has.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum ImageDimension {
    /// 1D image
    D1,
    /// 2D image
    D2,
    /// 3D image
    D3,
    /// Cube map
    Cube,
}

bitflags::bitflags! {
    /// Flags describing an image.
    #[cfg_attr(feature = "serialize", derive(Serialize))]
    #[cfg_attr(feature = "deserialize", derive(Deserialize))]
    pub struct ImageFlags: u32 {
        /// Image is an array.
        const ARRAYED = 0x1;
        /// Image is multisampled.
        const MULTISAMPLED = 0x2;
        /// Image is to be accessed with a sampler.
        const SAMPLED = 0x4;
        /// Image can be used as a source for load ops.
        const CAN_LOAD = 0x10;
        /// Image can be used as a target for store ops.
        const CAN_STORE = 0x20;
    }
}

/// A data type declared in the module.
#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct Type {
    /// The name of the type, if any.
    pub name: Option<String>,
    /// Inner structure that depends on the kind of the type.
    pub inner: TypeInner,
}

/// Enum with additional information, depending on the kind of type.
// Clone is used only for error reporting and is not intended for end users
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum TypeInner {
    /// Number of integral or floating-point kind.
    Scalar { kind: ScalarKind, width: Bytes },
    /// Vector of numbers.
    Vector {
        size: VectorSize,
        kind: ScalarKind,
        width: Bytes,
    },
    /// Matrix of numbers.
    Matrix {
        columns: VectorSize,
        rows: VectorSize,
        kind: ScalarKind,
        width: Bytes,
    },
    /// Pointer to a value.
    Pointer {
        base: Handle<Type>,
        class: StorageClass,
    },
    /// Homogenous list of elements.
    Array {
        base: Handle<Type>,
        size: ArraySize,
        stride: Option<NonZeroU32>,
    },
    /// User-defined structure.
    Struct { members: Vec<StructMember> },
    /// Possibly multidimensional array of texels.
    Image {
        base: Handle<Type>,
        dim: ImageDimension,
        flags: ImageFlags,
    },
    /// Depth-comparison image.
    DepthImage { dim: ImageDimension, arrayed: bool },
    /// Can be used to sample values from images.
    Sampler { comparison: bool },
}

/// Constant value.
#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct Constant {
    pub name: Option<String>,
    pub specialization: Option<u32>,
    pub inner: ConstantInner,
    pub ty: Handle<Type>,
}

/// Additional information, dependendent on the kind of constant.
// Clone is used only for error reporting and is not intended for end users
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum ConstantInner {
    Sint(i64),
    Uint(u64),
    Float(f64),
    Bool(bool),
    Composite(Vec<Handle<Constant>>),
}

/// Describes how an input/output variable is to be bound.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum Binding {
    /// Built-in shader variable.
    BuiltIn(BuiltIn),
    /// Indexed location.
    Location(u32),
    /// Binding within a descriptor set.
    Descriptor { set: u32, binding: u32 },
}

bitflags::bitflags! {
    /// Indicates how a global variable is used.
    #[cfg_attr(feature = "serialize", derive(Serialize))]
    #[cfg_attr(feature = "deserialize", derive(Deserialize))]
    pub struct GlobalUse: u8 {
        /// Data will be read from the variable.
        const LOAD = 0x1;
        /// Data will be written to the variable.
        const STORE = 0x2;
    }
}

/// Variable defined at module level.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct GlobalVariable {
    /// Name of the variable, if any.
    pub name: Option<String>,
    /// How this variable is to be stored.
    pub class: StorageClass,
    /// How this variable is to be bound.
    pub binding: Option<Binding>,
    /// The type of this variable.
    pub ty: Handle<Type>,
    /// The interpolation qualifier, if any.
    pub interpolation: Option<Interpolation>,
}

/// Variable defined at function level.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct LocalVariable {
    /// Name of the variable, if any.
    pub name: Option<String>,
    /// The type of this variable.
    pub ty: Handle<Type>,
    /// Initial value for this variable.
    pub init: Option<Handle<Expression>>,
}

/// Operation that can be applied on a single value.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum UnaryOperator {
    Negate,
    Not,
}

/// Operation that can be applied on two values.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    ExclusiveOr,
    InclusiveOr,
    LogicalAnd,
    LogicalOr,
    ShiftLeftLogical,
    ShiftRightLogical,
    ShiftRightArithmetic,
}

/// Built-in shader function.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum IntrinsicFunction {
    Any,
    All,
    IsNan,
    IsInf,
    IsFinite,
    IsNormal,
}

/// Axis on which to compute a derivative.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum DerivativeAxis {
    X,
    Y,
    Width,
}

/// Origin of a function to call.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum FunctionOrigin {
    Local(Handle<Function>),
    External(String),
}

/// An expression that can be evaluated to obtain a value.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum Expression {
    /// Array access with a computed index.
    Access {
        base: Handle<Expression>,
        index: Handle<Expression>, //int
    },
    /// Array access with a known index.
    AccessIndex {
        base: Handle<Expression>,
        index: u32,
    },
    /// Constant value.
    Constant(Handle<Constant>),
    /// Composite expression.
    Compose {
        ty: Handle<Type>,
        components: Vec<Handle<Expression>>,
    },
    /// Reference a function parameter, by its index.
    FunctionParameter(u32),
    /// Reference a global variable.
    GlobalVariable(Handle<GlobalVariable>),
    /// Reference a local variable.
    LocalVariable(Handle<LocalVariable>),
    /// Load a value indirectly.
    Load { pointer: Handle<Expression> },
    /// Sample a point from an image.
    ImageSample {
        image: Handle<Expression>,
        sampler: Handle<Expression>,
        coordinate: Handle<Expression>,
        depth_ref: Option<Handle<Expression>>,
    },
    /// Apply an unary operator.
    Unary {
        op: UnaryOperator,
        expr: Handle<Expression>,
    },
    /// Apply a binary operator.
    Binary {
        op: BinaryOperator,
        left: Handle<Expression>,
        right: Handle<Expression>,
    },
    /// Call an intrinsic function.
    Intrinsic {
        fun: IntrinsicFunction,
        argument: Handle<Expression>,
    },
    /// Dot product between two vectors.
    DotProduct(Handle<Expression>, Handle<Expression>),
    /// Cross product between two vectors.
    CrossProduct(Handle<Expression>, Handle<Expression>),
    /// Compute the derivative on an axis.
    Derivative {
        axis: DerivativeAxis,
        //modifier,
        expr: Handle<Expression>,
    },
    /// Call another function.
    Call {
        origin: FunctionOrigin,
        arguments: Vec<Handle<Expression>>,
    },
}

/// A code block is just a vector of statements.
pub type Block = Vec<Statement>;

/// Marker type, used for falling through in a switch statement.
// Clone is used only for error reporting and is not intended for end users
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct FallThrough;

/// Instructions which make up an executable block.
// Clone is used only for error reporting and is not intended for end users
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub enum Statement {
    /// Empty statement, does nothing.
    Empty,
    /// A block containing more statements, to be executed sequentially.
    Block(Block),
    /// Conditionally executes one of two blocks, based on the value of the condition.
    If {
        condition: Handle<Expression>, //bool
        accept: Block,
        reject: Block,
    },
    /// Conditionally executes one of multiple blocks, based on the value of the selector.
    Switch {
        selector: Handle<Expression>, //int
        cases: FastHashMap<i32, (Block, Option<FallThrough>)>,
        default: Block,
    },
    /// Executes a block repeatedly.
    Loop { body: Block, continuing: Block },
    //TODO: move terminator variations into a separate enum?
    /// Exits the loop.
    Break,
    /// Skips execution to the next iteration of the loop.
    Continue,
    /// Returns from the function (possibly with a value).
    Return { value: Option<Handle<Expression>> },
    /// Aborts the current shader execution.
    Kill,
    /// Stores a value at an address.
    Store {
        pointer: Handle<Expression>,
        value: Handle<Expression>,
    },
}

/// A function defined in the module.
#[derive(Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct Function {
    /// Name of the function, if any.
    pub name: Option<String>,
    //pub control: spirv::FunctionControl,
    /// The types of the parameters of this function.
    pub parameter_types: Vec<Handle<Type>>,
    /// The return type of this function, if any.
    pub return_type: Option<Handle<Type>>,
    /// Vector of global variable usages.
    ///
    /// Each item corresponds to a global variable in the module.
    pub global_usage: Vec<GlobalUse>,
    /// Local variables defined and used in the function.
    pub local_variables: Arena<LocalVariable>,
    /// Expressions used inside this function.
    pub expressions: Arena<Expression>,
    /// Block of instructions comprising the body of the function.
    pub body: Block,
}

/// Exported function, to be run at a certain stage in the pipeline.
#[derive(Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct EntryPoint {
    /// The stage in the programmable pipeline this entry point is for.
    pub stage: ShaderStage,
    /// Name identifying this entry point.
    pub name: String,
    /// The function to be used.
    pub function: Handle<Function>,
}

/// Shader module.
///
/// A module is a set of constants, global variables and functions, as well as
/// the types required to define them.
///
/// Some functions are marked as entry points, to be used in a certain shader stage.
///
/// To create a new module, use [`Module::from_header`] or [`Module::generate_empty`].
/// Alternatively, you can load an existing shader using one of the [available front ends][front].
///
/// When finished, you can export modules using one of the [available back ends][back].
#[derive(Debug)]
#[cfg_attr(feature = "serialize", derive(Serialize))]
#[cfg_attr(feature = "deserialize", derive(Deserialize))]
pub struct Module {
    /// Header containing module metadata.
    pub header: Header,
    /// Storage for the types defined in this module.
    pub types: Arena<Type>,
    /// Storage for the constants defined in this module.
    pub constants: Arena<Constant>,
    /// Storage for the global variables defined in this module.
    pub global_variables: Arena<GlobalVariable>,
    /// Storage for the functions defined in this module.
    pub functions: Arena<Function>,
    /// Vector of exported entry points.
    pub entry_points: Vec<EntryPoint>,
}
