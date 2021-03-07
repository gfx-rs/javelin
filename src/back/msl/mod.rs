/*! Metal Shading Language (MSL) backend

## Binding model

Metal's bindings are flat per resource. Since there isn't an obvious mapping
from SPIR-V's descriptor sets, we require a separate mapping provided in the options.
This mapping may have one or more resource end points for each descriptor set + index
pair.

## Entry points

Even though MSL and our IR appear to be similar in that the entry points in both can
accept arguments and return values, the restrictions are different.
MSL allows the varyings to be either in separate arguments, or inside a single
`[[stage_in]]` struct. We gather input varyings and form this artificial structure.
We also add all the (non-Private) globals into the arguments.

At the beginning of the entry point, we assign the local constants and re-compose
the arguments as they are declared on IR side, so that the rest of the logic can
pretend that MSL doesn't have all the restrictions it has.

For the result type, if it's a structure, we re-compose it with a temporary value
holding the result.
!*/

use crate::{
    arena::Handle,
    proc::{analyzer::Analysis, TypifyError},
    FastHashMap,
};
use std::{
    io::{Error as IoError, Write},
    string::FromUtf8Error,
};

mod keywords;
mod writer;

pub use writer::Writer;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindTarget {
    pub buffer: Option<u8>,
    pub texture: Option<u8>,
    pub sampler: Option<u8>,
    pub mutable: bool,
}

#[derive(Clone, Debug, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub struct BindSource {
    pub stage: crate::ShaderStage,
    pub group: u32,
    pub binding: u32,
}

pub type BindingMap = FastHashMap<BindSource, BindTarget>;

enum ResolvedBinding {
    BuiltIn(crate::BuiltIn),
    Attribute(u32),
    Color(u32),
    User { prefix: &'static str, index: u32 },
    Resource(BindTarget),
}

// Note: some of these should be removed in favor of proper IR validation.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    IO(#[from] IoError),
    #[error(transparent)]
    Utf8(#[from] FromUtf8Error),
    #[error(transparent)]
    Type(#[from] TypifyError),
    #[error("bind source for {0:?} is missing from the map")]
    MissingBindTarget(BindSource),
    #[error("bind target {0:?} is empty")]
    UnimplementedBindTarget(BindTarget),
    #[error("composing of {0:?} is not implemented yet")]
    UnsupportedCompose(Handle<crate::Type>),
    #[error("operation {0:?} is not implemented yet")]
    UnsupportedBinaryOp(crate::BinaryOperator),
    #[error("standard function '{0}' is not implemented yet")]
    UnsupportedCall(String),
    #[error("feature '{0}' is not implemented yet")]
    FeatureNotImplemented(String),
    #[error("module is not valid")]
    Validation,
}

#[derive(Clone, Copy, Debug)]
enum LocationMode {
    VertexInput,
    FragmentOutput,
    Intermediate,
    Uniform,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// (Major, Minor) target version of the Metal Shading Language.
    pub lang_version: (u8, u8),
    /// Binding model mapping to Metal.
    pub binding_map: BindingMap,
    /// Make it possible to link different stages via SPIRV-Cross.
    pub spirv_cross_compatibility: bool,
    /// Don't panic on missing bindings, instead generate invalid MSL.
    pub fake_missing_bindings: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            lang_version: (1, 0),
            binding_map: BindingMap::default(),
            spirv_cross_compatibility: false,
            fake_missing_bindings: true,
        }
    }
}

impl Options {
    fn resolve_local_binding(
        &self,
        binding: &crate::Binding,
        mode: LocationMode,
    ) -> Result<ResolvedBinding, Error> {
        match *binding {
            crate::Binding::BuiltIn(built_in) => Ok(ResolvedBinding::BuiltIn(built_in)),
            crate::Binding::Location(index, _) => match mode {
                LocationMode::VertexInput => Ok(ResolvedBinding::Attribute(index)),
                LocationMode::FragmentOutput => Ok(ResolvedBinding::Color(index)),
                LocationMode::Intermediate => Ok(ResolvedBinding::User {
                    prefix: if self.spirv_cross_compatibility {
                        "locn"
                    } else {
                        "loc"
                    },
                    index,
                }),
                LocationMode::Uniform => {
                    log::error!(
                        "Unexpected Binding::Location({}) for the Uniform mode",
                        index
                    );
                    Err(Error::Validation)
                }
            },
        }
    }

    fn resolve_global_binding(
        &self,
        stage: crate::ShaderStage,
        res_binding: &crate::ResourceBinding,
    ) -> Result<ResolvedBinding, Error> {
        let source = BindSource {
            stage,
            group: res_binding.group,
            binding: res_binding.binding,
        };
        match self.binding_map.get(&source) {
            Some(target) => Ok(ResolvedBinding::Resource(target.clone())),
            None if self.fake_missing_bindings => Ok(ResolvedBinding::User {
                prefix: "fake",
                index: 0,
            }),
            None => Err(Error::MissingBindTarget(source)),
        }
    }
}

impl ResolvedBinding {
    fn try_fmt<W: Write>(&self, out: &mut W) -> Result<(), Error> {
        match *self {
            ResolvedBinding::BuiltIn(built_in) => {
                use crate::BuiltIn as Bi;
                let name = match built_in {
                    // vertex
                    Bi::BaseInstance => "base_instance",
                    Bi::BaseVertex => "base_vertex",
                    Bi::ClipDistance => "clip_distance",
                    Bi::InstanceIndex => "instance_id",
                    Bi::PointSize => "point_size",
                    Bi::Position => "position",
                    Bi::VertexIndex => "vertex_id",
                    // fragment
                    Bi::FragCoord => "position",
                    Bi::FragDepth => "depth(any)",
                    Bi::FrontFacing => "front_facing",
                    Bi::SampleIndex => "sample_id",
                    Bi::SampleMaskIn => "sample_mask",
                    Bi::SampleMaskOut => "sample_mask",
                    // compute
                    Bi::GlobalInvocationId => "thread_position_in_grid",
                    Bi::LocalInvocationId => "thread_position_in_threadgroup",
                    Bi::LocalInvocationIndex => "thread_index_in_threadgroup",
                    Bi::WorkGroupId => "threadgroup_position_in_grid",
                    Bi::WorkGroupSize => "dispatch_threads_per_threadgroup",
                };
                Ok(write!(out, "{}", name)?)
            }
            ResolvedBinding::Attribute(index) => Ok(write!(out, "attribute({})", index)?),
            ResolvedBinding::Color(index) => Ok(write!(out, "color({})", index)?),
            ResolvedBinding::User { prefix, index } => {
                Ok(write!(out, "user({}{})", prefix, index)?)
            }
            ResolvedBinding::Resource(ref target) => {
                if let Some(id) = target.buffer {
                    Ok(write!(out, "buffer({})", id)?)
                } else if let Some(id) = target.texture {
                    Ok(write!(out, "texture({})", id)?)
                } else if let Some(id) = target.sampler {
                    Ok(write!(out, "sampler({})", id)?)
                } else {
                    Err(Error::UnimplementedBindTarget(target.clone()))
                }
            }
        }
    }

    fn try_fmt_decorated<W: Write>(&self, out: &mut W, terminator: &str) -> Result<(), Error> {
        write!(out, " [[")?;
        self.try_fmt(out)?;
        write!(out, "]]")?;
        write!(out, "{}", terminator)?;
        Ok(())
    }
}

/// Information about a translated module that is required
/// for the use of the result.
pub struct TranslationInfo {
    /// Mapping of the entry point names. Each item in the array
    /// corresponds to an entry point in `module.entry_points.iter()`.
    pub entry_point_names: Vec<String>,
}

pub fn write_string(
    module: &crate::Module,
    analysis: &Analysis,
    options: &Options,
) -> Result<(String, TranslationInfo), Error> {
    let mut w = writer::Writer::new(Vec::new());
    let info = w.write(module, analysis, options)?;
    let string = String::from_utf8(w.finish())?;
    Ok((string, info))
}
