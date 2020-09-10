use super::{BorrowType, MaybeOwned};
use crate::{
    Arena, ArraySize, BinaryOperator, BuiltIn, Constant, ConstantInner, DerivativeAxis, Expression,
    FastHashMap, Function, FunctionOrigin, GlobalVariable, Handle, ImageClass, Interpolation,
    IntrinsicFunction, LocalVariable, MemberOrigin, Module, ScalarKind, ShaderStage, Statement,
    StorageAccess, StorageClass, StorageFormat, StructMember, Type, TypeInner, UnaryOperator,
};
use log::warn;
use std::{
    borrow::Cow,
    fmt::{self, Error as FmtError, Write as FmtWrite},
    io::{Error as IoError, Write},
};

#[derive(Debug)]
pub enum Error {
    FormatError(FmtError),
    IoError(IoError),
    Custom(String),
}

impl From<FmtError> for Error {
    fn from(err: FmtError) -> Self {
        Error::FormatError(err)
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Self {
        Error::IoError(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::FormatError(err) => write!(f, "Formatting error {}", err),
            Error::IoError(err) => write!(f, "Io error: {}", err),
            Error::Custom(err) => write!(f, "{}", err),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Version {
    Desktop(u16),
    Embedded(u16),
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Version::Desktop(v) => write!(f, "{} core", v),
            Version::Embedded(v) => write!(f, "{} es", v),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub version: Version,
    pub entry_point: (String, ShaderStage),
}

#[derive(Debug, Clone)]
pub struct TextureMapping {
    pub texture: Handle<GlobalVariable>,
    pub sampler: Handle<GlobalVariable>,
}

const SUPPORTED_CORE_VERSIONS: &[u16] = &[450, 460];
const SUPPORTED_ES_VERSIONS: &[u16] = &[300, 310];

bitflags::bitflags! {
    struct SupportedFeatures: u32 {
        const BUFFER_STORAGE = 1;
        const SHARED_STORAGE = 1 << 1;
        const DOUBLE_TYPE = 1 << 2;
        const NON_FLOAT_MATRICES = 1 << 3;
        const MULTISAMPLED_TEXTURES = 1 << 4;
        const MULTISAMPLED_TEXTURE_ARRAYS = 1 << 5;
        const NON_2D_TEXTURE_ARRAYS = 1 << 6;
    }
}

pub fn write<'a>(
    module: &'a Module,
    out: &mut impl Write,
    options: Options,
) -> Result<FastHashMap<String, TextureMapping>, Error> {
    let (version, es) = match options.version {
        Version::Desktop(v) => (v, false),
        Version::Embedded(v) => (v, true),
    };

    if (!es && !SUPPORTED_CORE_VERSIONS.contains(&version))
        || (es && !SUPPORTED_ES_VERSIONS.contains(&version))
    {
        return Err(Error::Custom(format!(
            "Version not supported {}",
            options.version
        )));
    }

    writeln!(out, "#version {}\n", options.version)?;

    if es {
        writeln!(out, "precision highp float;\n")?;
    }

    let mut counter = 0;
    let mut names = FastHashMap::default();

    let mut namer = |name: Option<&'a String>| {
        if let Some(name) = name {
            if !is_valid_ident(name) || names.get(name.as_str()).is_some() {
                counter += 1;
                while names.get(format!("_{}", counter).as_str()).is_some() {
                    counter += 1;
                }
                format!("_{}", counter)
            } else {
                names.insert(name.as_str(), ());
                name.clone()
            }
        } else {
            counter += 1;
            while names.get(format!("_{}", counter).as_str()).is_some() {
                counter += 1;
            }
            format!("_{}", counter)
        }
    };

    let entry_point = module
        .entry_points
        .iter()
        .find(|entry| entry.name == options.entry_point.0 && entry.stage == options.entry_point.1)
        .ok_or_else(|| Error::Custom(String::from("Entry point not found")))?;
    let func = &module.functions[entry_point.function];

    if let ShaderStage::Compute { .. } = entry_point.stage {
        if (es && version < 310) || (!es && version < 430) {
            return Err(Error::Custom(format!(
                "Version {} doesn't support compute shaders",
                options.version
            )));
        }

        if !es && version < 460 {
            writeln!(out, "#extension ARB_compute_shader : require")?;
        }
    }

    let mut features = SupportedFeatures::empty();

    if !es && version > 440 {
        features |= SupportedFeatures::DOUBLE_TYPE;
        features |= SupportedFeatures::NON_FLOAT_MATRICES;
        features |= SupportedFeatures::MULTISAMPLED_TEXTURE_ARRAYS;
        features |= SupportedFeatures::NON_2D_TEXTURE_ARRAYS;
    }

    if !es || version > 300 {
        features |= SupportedFeatures::BUFFER_STORAGE;
        features |= SupportedFeatures::SHARED_STORAGE;
        features |= SupportedFeatures::MULTISAMPLED_TEXTURES;
    }

    let mut structs = FastHashMap::default();
    let mut built_structs = FastHashMap::default();

    // Do a first pass to collect names
    for (handle, ty) in module.types.iter() {
        match ty.inner {
            TypeInner::Struct { .. } => {
                let name = namer(ty.name.as_ref());

                structs.insert(handle, name);
            }
            _ => continue,
        }
    }

    for ((_, global), usage) in module.global_variables.iter().zip(func.global_usage.iter()) {
        if usage.is_empty() {
            continue;
        }

        let block = match global.class {
            StorageClass::Input
            | StorageClass::Output
            | StorageClass::StorageBuffer
            | StorageClass::Uniform => true,
            _ => false,
        };

        match module.types[global.ty].inner {
            TypeInner::Struct { .. } if block => {
                built_structs.insert(global.ty, ());
            }
            _ => {}
        }
    }

    // Do a second pass to build the structs
    for (handle, ty) in module.types.iter() {
        match ty.inner {
            TypeInner::Struct { ref members } => {
                write_struct(
                    handle,
                    members,
                    module,
                    &structs,
                    out,
                    &mut built_structs,
                    features,
                )?;
            }
            _ => continue,
        }
    }

    writeln!(out)?;

    let mut functions = FastHashMap::default();

    for (handle, func) in module.functions.iter() {
        // Discard all entry points
        if entry_point.function != handle
            && module
                .entry_points
                .iter()
                .any(|entry| entry.function == handle)
        {
            continue;
        }

        let name = if entry_point.function != handle {
            namer(func.name.as_ref())
        } else {
            String::from("main")
        };

        writeln!(
            out,
            "{} {}({});",
            func.return_type
                .map(|ty| write_type(ty, &module.types, &structs, None, features))
                .transpose()?
                .as_deref()
                .unwrap_or("void"),
            name,
            func.parameter_types
                .iter()
                .map(|ty| write_type(*ty, &module.types, &structs, None, features))
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        )?;

        functions.insert(handle, name);
    }

    writeln!(out)?;

    let texture_mappings = collect_texture_mapping(module, &functions)?;
    let mut mappings_map = FastHashMap::default();

    for ((handle, global), _) in module
        .global_variables
        .iter()
        .zip(func.global_usage.iter())
        .filter(|(_, usage)| !usage.is_empty())
    {
        if let TypeInner::Image {
            kind,
            dim,
            arrayed,
            class,
        } = module.types[global.ty].inner
        {
            let mapping =
                if let Some(map) = texture_mappings.iter().find(|map| map.texture == handle) {
                    map
                } else {
                    warn!(
                        "Couldn't find a mapping for {:?}, handle {:?}",
                        global, handle
                    );
                    continue;
                };

            if let Some(ref binding) = global.binding {
                write!(out, "layout(")?;

                if !es {
                    write!(out, "{}", Binding(binding))?;

                    write!(out, ",")?;
                }

                if let TypeInner::Image {
                    class: ImageClass::Storage(storage_format),
                    ..
                } = module.types[global.ty].inner
                {
                    write!(out, "{}) ", write_format_glsl(storage_format),)?;
                } else {
                    write!(out, ") ")?;
                }

                if global.storage_access == StorageAccess::LOAD {
                    write!(out, "readonly ")?;
                } else if global.storage_access == StorageAccess::STORE {
                    write!(out, "writeonly ")?;
                }
            }

            let name = if !es {
                namer(global.name.as_ref())
            } else {
                global.name.clone().ok_or_else(|| {
                    Error::Custom(String::from("Global names must be specified in es"))
                })?
            };

            let comparison = if let TypeInner::Sampler { comparison } =
                module.types[module.global_variables[mapping.sampler].ty].inner
            {
                comparison
            } else {
                unreachable!()
            };

            writeln!(
                out,
                "{}{} {};",
                write_storage_class(global.class, features)?,
                write_image_type(kind, dim, arrayed, class, comparison, features)?,
                name
            )?;

            mappings_map.insert(name, mapping.clone());
        }
    }

    let mut globals_lookup = FastHashMap::default();

    for ((handle, global), _) in module
        .global_variables
        .iter()
        .zip(func.global_usage.iter())
        .filter(|(_, usage)| !usage.is_empty())
    {
        match module.types[global.ty].inner {
            TypeInner::Image { .. } | TypeInner::Sampler { .. } => continue,
            _ => {}
        }

        if let Some(crate::Binding::BuiltIn(built_in)) = global.binding {
            let semantic = match built_in {
                BuiltIn::Position => "gl_Position",
                BuiltIn::GlobalInvocationId => "gl_GlobalInvocationID",
                BuiltIn::BaseInstance => "gl_BaseInstance",
                BuiltIn::BaseVertex => "gl_BaseVertex",
                BuiltIn::ClipDistance => "gl_ClipDistance",
                BuiltIn::InstanceIndex => "gl_InstanceIndex",
                BuiltIn::VertexIndex => "gl_VertexIndex",
                BuiltIn::PointSize => "gl_PointSize",
                BuiltIn::FragCoord => "gl_FragCoord",
                BuiltIn::FrontFacing => "gl_FrontFacing",
                BuiltIn::SampleIndex => "gl_SampleID",
                BuiltIn::FragDepth => "gl_FragDepth",
                BuiltIn::LocalInvocationId => "gl_LocalInvocationID",
                BuiltIn::LocalInvocationIndex => "gl_LocalInvocationIndex",
                BuiltIn::WorkGroupId => "gl_WorkGroupID",
            };

            globals_lookup.insert(handle, String::from(semantic));
            continue;
        }

        let name = if !es {
            namer(global.name.as_ref())
        } else {
            global.name.clone().ok_or_else(|| {
                Error::Custom(String::from("Global names must be specified in es"))
            })?
        };

        let storage_format = match module.types[global.ty].inner {
            TypeInner::Image {
                class: ImageClass::Storage(format),
                ..
            } => Some(format),
            _ => None,
        };

        if global.binding.is_some() {
            write!(out, "layout(")?;

            if let Some(ref binding) = global.binding {
                if !es {
                    write!(out, "{}", Binding(binding))?;
                }
            }

            if storage_format.is_some() && global.binding.is_some() {
                write!(out, ",")?;
            }

            if let Some(format) = storage_format {
                write!(out, "{}", write_format_glsl(format))?;
            }

            write!(out, ") ")?;
        }

        if global.storage_access == StorageAccess::LOAD {
            write!(out, "readonly ")?;
        } else if global.storage_access == StorageAccess::STORE {
            write!(out, "writeonly ")?;
        }

        if let Some(interpolation) = global.interpolation {
            match (entry_point.stage, global.class) {
                (ShaderStage::Fragment { .. }, StorageClass::Input)
                | (ShaderStage::Vertex, StorageClass::Output) => {
                    write!(out, "{} ", write_interpolation(interpolation)?)?;
                }
                _ => {}
            };
        }

        let block = match global.class {
            StorageClass::Input
            | StorageClass::Output
            | StorageClass::StorageBuffer
            | StorageClass::Uniform => Some(namer(None)),
            _ => None,
        };

        writeln!(
            out,
            "{}{} {};",
            write_storage_class(global.class, features)?,
            write_type(global.ty, &module.types, &structs, block, features)?,
            name
        )?;

        globals_lookup.insert(handle, name);
    }

    writeln!(out)?;

    for (handle, name) in functions.iter() {
        let func = &module.functions[*handle];

        let args: FastHashMap<_, _> = func
            .parameter_types
            .iter()
            .enumerate()
            .map(|(pos, ty)| (pos as u32, (namer(None), *ty)))
            .collect();

        writeln!(
            out,
            "{} {}({}) {{",
            func.return_type
                .map(|ty| write_type(ty, &module.types, &structs, None, features))
                .transpose()?
                .as_deref()
                .unwrap_or("void"),
            name,
            args.values()
                .map::<Result<_, Error>, _>(|(name, ty)| {
                    let ty = write_type(*ty, &module.types, &structs, None, features)?;

                    Ok(format!("{} {}", ty, name))
                })
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        )?;

        let locals: FastHashMap<_, _> = func
            .local_variables
            .iter()
            .map(|(handle, local)| (handle, namer(local.name.as_ref())))
            .collect();

        for (handle, name) in locals.iter() {
            writeln!(
                out,
                "\t{} {};",
                write_type(
                    func.local_variables[*handle].ty,
                    &module.types,
                    &structs,
                    None,
                    features
                )?,
                name
            )?;
        }

        writeln!(out)?;

        let mut builder = StatementBuilder {
            functions: &functions,
            globals: &globals_lookup,
            locals_lookup: &locals,
            structs: &structs,
            args: &args,
            expressions: &func.expressions,
            locals: &func.local_variables,
            features,
        };

        for sta in func.body.iter() {
            writeln!(out, "{}", write_statement(sta, module, &mut builder, 1)?)?;
        }

        writeln!(out, "}}")?;
    }

    Ok(mappings_map)
}

struct Binding<'a>(&'a crate::Binding);
impl<'a> fmt::Display for Binding<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            crate::Binding::BuiltIn(_) => write!(f, ""), // Ignore because they are variables with a predefined name
            crate::Binding::Location(location) => write!(f, "location={}", location),
            crate::Binding::Descriptor { set, binding } => {
                write!(f, "set={},binding={}", set, binding)
            }
        }
    }
}

struct StatementBuilder<'a> {
    pub functions: &'a FastHashMap<Handle<Function>, String>,
    pub globals: &'a FastHashMap<Handle<GlobalVariable>, String>,
    pub locals_lookup: &'a FastHashMap<Handle<LocalVariable>, String>,
    pub structs: &'a FastHashMap<Handle<Type>, String>,
    pub args: &'a FastHashMap<u32, (String, Handle<Type>)>,
    pub expressions: &'a Arena<Expression>,
    pub locals: &'a Arena<LocalVariable>,
    pub features: SupportedFeatures,
}

fn write_statement<'a, 'b>(
    sta: &Statement,
    module: &'a Module,
    builder: &'b mut StatementBuilder<'a>,
    indent: usize,
) -> Result<String, Error> {
    Ok(match sta {
        Statement::Empty => String::new(),
        Statement::Block(block) => block
            .iter()
            .map(|sta| write_statement(sta, module, builder, indent))
            .collect::<Result<Vec<_>, _>>()?
            .join("\n"),
        Statement::If {
            condition,
            accept,
            reject,
        } => {
            let mut out = String::new();

            writeln!(
                &mut out,
                "{}if({}) {{",
                "\t".repeat(indent),
                write_expression(&builder.expressions[*condition], module, builder)?.0
            )?;

            for sta in accept {
                writeln!(
                    &mut out,
                    "{}",
                    write_statement(sta, module, builder, indent + 1)?
                )?;
            }

            if !reject.is_empty() {
                writeln!(&mut out, "{}}} else {{", "\t".repeat(indent),)?;
                for sta in reject {
                    writeln!(
                        &mut out,
                        "{}",
                        write_statement(sta, module, builder, indent + 1)?
                    )?;
                }
            }

            write!(&mut out, "{}}}", "\t".repeat(indent),)?;

            out
        }
        Statement::Switch {
            selector,
            cases,
            default,
        } => {
            let mut out = String::new();

            writeln!(
                &mut out,
                "{}switch({}) {{",
                "\t".repeat(indent),
                write_expression(&builder.expressions[*selector], module, builder)?.0
            )?;

            for (label, (block, fallthrough)) in cases {
                writeln!(&mut out, "{}case {}:", "\t".repeat(indent + 1), label)?;

                for sta in block {
                    writeln!(
                        &mut out,
                        "{}",
                        write_statement(sta, module, builder, indent + 2)?
                    )?;
                }

                if fallthrough.is_none() {
                    writeln!(&mut out, "{}break;", "\t".repeat(indent + 2),)?;
                }
            }

            if !default.is_empty() {
                writeln!(&mut out, "{}default:", "\t".repeat(indent + 1),)?;

                for sta in default {
                    writeln!(
                        &mut out,
                        "{}",
                        write_statement(sta, module, builder, indent + 2)?
                    )?;
                }
            }

            write!(&mut out, "{}}}", "\t".repeat(indent),)?;

            out
        }
        Statement::Loop { body, continuing } => {
            let mut out = String::new();

            writeln!(&mut out, "{}while(true) {{", "\t".repeat(indent),)?;

            for sta in body.iter().chain(continuing.iter()) {
                writeln!(
                    &mut out,
                    "{}",
                    write_statement(sta, module, builder, indent + 1)?
                )?;
            }

            write!(&mut out, "{}}}", "\t".repeat(indent),)?;

            out
        }
        Statement::Break => format!("{}break;", "\t".repeat(indent),),
        Statement::Continue => format!("{}continue;", "\t".repeat(indent),),
        Statement::Return { value } => format!(
            "{}{}",
            "\t".repeat(indent),
            if let Some(expr) = value {
                format!(
                    "return {};",
                    write_expression(&builder.expressions[*expr], module, builder)?.0
                )
            } else {
                String::from("return;")
            }
        ),
        Statement::Kill => format!("{}discard;", "\t".repeat(indent)),
        Statement::Store { pointer, value } => format!(
            "{}{} = {};",
            "\t".repeat(indent),
            write_expression(&builder.expressions[*pointer], module, builder)?.0,
            write_expression(&builder.expressions[*value], module, builder)?.0
        ),
    })
}

fn write_expression<'a, 'b>(
    expr: &Expression,
    module: &'a Module,
    builder: &'b mut StatementBuilder<'a>,
) -> Result<(Cow<'a, str>, BorrowType<'a>), Error> {
    Ok(match *expr {
        Expression::Access { base, index } => {
            let (base_expr, ty) = write_expression(&builder.expressions[base], module, builder)?;

            let inner = match *ty.borrow() {
                TypeInner::Vector { kind, width, .. } => {
                    MaybeOwned::Owned(TypeInner::Scalar { kind, width })
                }
                TypeInner::Matrix {
                    kind,
                    width,
                    columns,
                    ..
                } => MaybeOwned::Owned(TypeInner::Vector {
                    kind,
                    width,
                    size: columns,
                }),
                TypeInner::Array { base, .. } => module.borrow_type(base),
                _ => return Err(Error::Custom(format!("Cannot dynamically index {:?}", ty))),
            };

            (
                Cow::Owned(format!(
                    "{}[{}]",
                    base_expr,
                    write_expression(&builder.expressions[index], module, builder)?.0
                )),
                inner,
            )
        }
        Expression::AccessIndex { base, index } => {
            let (base_expr, ty) = write_expression(&builder.expressions[base], module, builder)?;

            match *ty.borrow() {
                TypeInner::Vector { kind, width, .. } => (
                    Cow::Owned(format!("{}[{}]", base_expr, index)),
                    MaybeOwned::Owned(TypeInner::Scalar { kind, width }),
                ),
                TypeInner::Matrix {
                    kind,
                    width,
                    columns,
                    ..
                } => (
                    Cow::Owned(format!("{}[{}]", base_expr, index)),
                    MaybeOwned::Owned(TypeInner::Vector {
                        kind,
                        width,
                        size: columns,
                    }),
                ),
                TypeInner::Array { base, .. } => (
                    Cow::Owned(format!("{}[{}]", base_expr, index)),
                    module.borrow_type(base),
                ),
                TypeInner::Struct { ref members } => (
                    if let MemberOrigin::BuiltIn(builtin) = members[index as usize].origin {
                        Cow::Borrowed(builtin_to_glsl(builtin))
                    } else {
                        Cow::Owned(format!(
                            "{}.{}",
                            base_expr,
                            members[index as usize]
                                .name
                                .as_ref()
                                .filter(|s| is_valid_ident(s))
                                .unwrap_or(&format!("_{}", index))
                        ))
                    },
                    module.borrow_type(members[index as usize].ty),
                ),
                _ => return Err(Error::Custom(format!("Cannot index {:?}", ty))),
            }
        }
        Expression::Constant(constant) => (
            Cow::Owned(write_constant(
                &module.constants[constant],
                module,
                builder,
                builder.features,
            )?),
            module.borrow_type(module.constants[constant].ty),
        ),
        Expression::Compose { ty, ref components } => {
            let constructor = match module.types[ty].inner {
                TypeInner::Vector { size, kind, width } => format!(
                    "{}vec{}",
                    map_scalar(kind, width, builder.features)?.prefix,
                    size as u8,
                ),
                TypeInner::Matrix {
                    columns,
                    rows,
                    kind,
                    width,
                } => format!(
                    "{}mat{}x{}",
                    map_scalar(kind, width, builder.features)?.prefix,
                    columns as u8,
                    rows as u8,
                ),
                TypeInner::Array { .. } => {
                    write_type(ty, &module.types, builder.structs, None, builder.features)?
                        .into_owned()
                }
                TypeInner::Struct { .. } => builder.structs.get(&ty).unwrap().clone(),
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot compose type {}",
                        write_type(ty, &module.types, builder.structs, None, builder.features)?
                    )))
                }
            };

            (
                Cow::Owned(format!(
                    "{}({})",
                    constructor,
                    components
                        .iter()
                        .map::<Result<_, Error>, _>(|arg| Ok(write_expression(
                            &builder.expressions[*arg],
                            module,
                            builder
                        )?
                        .0))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                )),
                module.borrow_type(ty),
            )
        }
        Expression::FunctionParameter(pos) => {
            let (arg, ty) = builder.args.get(&pos).unwrap();

            (Cow::Borrowed(arg), module.borrow_type(*ty))
        }
        Expression::GlobalVariable(handle) => (
            Cow::Borrowed(builder.globals.get(&handle).unwrap()),
            module.borrow_type(module.global_variables[handle].ty),
        ),
        Expression::LocalVariable(handle) => (
            Cow::Borrowed(builder.locals_lookup.get(&handle).unwrap()),
            module.borrow_type(builder.locals[handle].ty),
        ),
        Expression::Load { pointer } => {
            write_expression(&builder.expressions[pointer], module, builder)?
        }
        Expression::ImageSample {
            image,
            sampler,
            coordinate,
            level,
            depth_ref,
        } => {
            let (image_expr, image_ty) =
                write_expression(&builder.expressions[image], module, builder)?;
            let (sampler_expr, sampler_ty) =
                write_expression(&builder.expressions[sampler], module, builder)?;
            let (coordinate_expr, coordinate_ty) =
                write_expression(&builder.expressions[coordinate], module, builder)?;

            let (kind, dim, arrayed, class) = match *image_ty.borrow() {
                TypeInner::Image {
                    kind,
                    dim,
                    arrayed,
                    class,
                } => (kind, dim, arrayed, class),
                _ => return Err(Error::Custom(format!("Cannot sample {:?}", image_ty))),
            };

            let ms = match class {
                crate::ImageClass::Multisampled => true,
                _ => false,
            };
            let shadow = match *sampler_ty.borrow() {
                TypeInner::Sampler { comparison } => comparison,
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot have a sampler of {:?}",
                        sampler_ty
                    )))
                }
            };
            let size = match *coordinate_ty.borrow() {
                TypeInner::Vector { size, .. } => size,
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot sample with coordinates of type {:?}",
                        coordinate_ty
                    )))
                }
            };

            let sampler_constructor = format!(
                "{}sampler{}{}{}{}({},{})",
                map_scalar(kind, 4, builder.features)?.prefix,
                ImageDimension(dim),
                if ms { "MS" } else { "" },
                if arrayed { "Array" } else { "" },
                if shadow { "Shadow" } else { "" },
                image_expr,
                sampler_expr
            );

            let coordinate = if let Some(depth_ref) = depth_ref {
                Cow::Owned(format!(
                    "vec{}({},{})",
                    size as u8 + 1,
                    coordinate_expr,
                    write_expression(&builder.expressions[depth_ref], module, builder)?.0
                ))
            } else {
                coordinate_expr
            };

            //TODO: handle MS
            let expr = match level {
                crate::SampleLevel::Auto => {
                    format!("texture({},{})", sampler_constructor, coordinate)
                }
                crate::SampleLevel::Exact(expr) => {
                    let (level_expr, _) =
                        write_expression(&builder.expressions[expr], module, builder)?;
                    format!(
                        "textureLod({}, {}, {})",
                        sampler_constructor, coordinate, level_expr
                    )
                }
                crate::SampleLevel::Bias(bias) => {
                    let (bias_expr, _) =
                        write_expression(&builder.expressions[bias], module, builder)?;
                    format!(
                        "texture({},{},{})",
                        sampler_constructor, coordinate, bias_expr
                    )
                }
            };

            let width = 4;
            let ty = if shadow {
                MaybeOwned::Owned(TypeInner::Scalar { kind, width })
            } else {
                MaybeOwned::Owned(TypeInner::Vector { kind, width, size })
            };

            (Cow::Owned(expr), ty)
        }
        Expression::ImageLoad {
            image,
            coordinate,
            index,
        } => {
            let (image_expr, image_ty) =
                write_expression(&builder.expressions[image], module, builder)?;
            let (coordinate_expr, coordinate_ty) =
                write_expression(&builder.expressions[coordinate], module, builder)?;

            let (kind, dim, arrayed, class) = match *image_ty.borrow() {
                TypeInner::Image {
                    kind,
                    dim,
                    arrayed,
                    class,
                } => (kind, dim, arrayed, class),
                _ => return Err(Error::Custom(format!("Cannot load {:?}", image_ty))),
            };

            let size = match *coordinate_ty.borrow() {
                TypeInner::Vector { size, .. } => size,
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot sample with coordinates of type {:?}",
                        coordinate_ty
                    )))
                }
            };

            let expr = match class {
                ImageClass::Sampled | ImageClass::Multisampled => {
                    let ms = match class {
                        crate::ImageClass::Multisampled => true,
                        _ => false,
                    };

                    //TODO: fix this
                    let sampler_constructor = format!(
                        "{}sampler{}{}{}({})",
                        map_scalar(kind, 4, builder.features)?.prefix,
                        ImageDimension(dim),
                        if ms { "MS" } else { "" },
                        if arrayed { "Array" } else { "" },
                        image_expr,
                    );

                    if !ms {
                        format!("texelFetch({},{})", sampler_constructor, coordinate_expr)
                    } else {
                        let (index_expr, _) =
                            write_expression(&builder.expressions[index], module, builder)?;

                        format!(
                            "texelFetch({},{},{})",
                            sampler_constructor, coordinate_expr, index_expr
                        )
                    }
                }
                ImageClass::Storage(_) => format!("imageLoad({},{})", image_expr, coordinate_expr),
                ImageClass::Depth => todo!(),
            };

            let width = 4;
            (
                Cow::Owned(expr),
                MaybeOwned::Owned(TypeInner::Vector { kind, width, size }),
            )
        }
        Expression::Unary { op, expr } => {
            let (expr, ty) = write_expression(&builder.expressions[expr], module, builder)?;

            (
                Cow::Owned(format!(
                    "({} {})",
                    match op {
                        UnaryOperator::Negate => "-",
                        UnaryOperator::Not => match ty.borrow() {
                            TypeInner::Scalar {
                                kind: ScalarKind::Sint,
                                ..
                            } => "~",
                            TypeInner::Scalar {
                                kind: ScalarKind::Uint,
                                ..
                            } => "~",
                            TypeInner::Scalar {
                                kind: ScalarKind::Bool,
                                ..
                            } => "!",
                            _ =>
                                return Err(Error::Custom(format!(
                                    "Cannot apply not to type {:?}",
                                    ty
                                ))),
                        },
                    },
                    expr
                )),
                ty,
            )
        }
        Expression::Binary { op, left, right } => {
            let (left_expr, left_ty) =
                write_expression(&builder.expressions[left], module, builder)?;
            let (right_expr, right_ty) =
                write_expression(&builder.expressions[right], module, builder)?;

            let op_str = match op {
                BinaryOperator::Add => "+",
                BinaryOperator::Subtract => "-",
                BinaryOperator::Multiply => "*",
                BinaryOperator::Divide => "/",
                BinaryOperator::Modulo => "%",
                BinaryOperator::Equal => "==",
                BinaryOperator::NotEqual => "!=",
                BinaryOperator::Less => "<",
                BinaryOperator::LessEqual => "<=",
                BinaryOperator::Greater => ">",
                BinaryOperator::GreaterEqual => ">=",
                BinaryOperator::And => "&",
                BinaryOperator::ExclusiveOr => "^",
                BinaryOperator::InclusiveOr => "|",
                BinaryOperator::LogicalAnd => "&&",
                BinaryOperator::LogicalOr => "||",
                BinaryOperator::ShiftLeftLogical => "<<",
                BinaryOperator::ShiftRightLogical => todo!(),
                BinaryOperator::ShiftRightArithmetic => ">>",
            };

            let ty = match op {
                BinaryOperator::Add
                | BinaryOperator::Subtract
                | BinaryOperator::Multiply
                | BinaryOperator::Divide
                | BinaryOperator::Modulo
                | BinaryOperator::And
                | BinaryOperator::ExclusiveOr
                | BinaryOperator::InclusiveOr
                | BinaryOperator::LogicalAnd
                | BinaryOperator::LogicalOr
                | BinaryOperator::ShiftLeftLogical
                | BinaryOperator::ShiftRightLogical
                | BinaryOperator::ShiftRightArithmetic => {
                    match (left_ty.borrow(), right_ty.borrow()) {
                        (TypeInner::Scalar { .. }, TypeInner::Scalar { .. }) => left_ty,
                        (TypeInner::Scalar { .. }, TypeInner::Vector { .. }) => right_ty,
                        (TypeInner::Scalar { .. }, TypeInner::Matrix { .. }) => right_ty,
                        (TypeInner::Vector { .. }, TypeInner::Scalar { .. }) => left_ty,
                        (TypeInner::Vector { .. }, TypeInner::Vector { .. }) => left_ty,
                        (TypeInner::Vector { .. }, TypeInner::Matrix { .. }) => left_ty,
                        (TypeInner::Matrix { .. }, TypeInner::Scalar { .. }) => left_ty,
                        (TypeInner::Matrix { .. }, TypeInner::Vector { .. }) => right_ty,
                        (TypeInner::Matrix { .. }, TypeInner::Matrix { .. }) => left_ty,
                        _ => {
                            return Err(Error::Custom(format!(
                                "Cannot apply '{}' to {} and {}",
                                op_str, left_expr, right_expr
                            )))
                        }
                    }
                }
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual => MaybeOwned::Owned(TypeInner::Scalar {
                    kind: ScalarKind::Bool,
                    width: 1,
                }),
            };

            (
                Cow::Owned(format!("({} {} {})", left_expr, op_str, right_expr)),
                ty,
            )
        }
        Expression::Intrinsic { fun, argument } => {
            let (expr, ty) = write_expression(&builder.expressions[argument], module, builder)?;

            (
                Cow::Owned(format!(
                    "{:?}({})",
                    match fun {
                        IntrinsicFunction::IsFinite => "!isinf",
                        IntrinsicFunction::IsInf => "isinf",
                        IntrinsicFunction::IsNan => "isnan",
                        IntrinsicFunction::IsNormal => "!isnan",
                        IntrinsicFunction::All => "all",
                        IntrinsicFunction::Any => "any",
                    },
                    expr
                )),
                ty,
            )
        }
        Expression::Transpose(matrix) => {
            let (matrix_expr, matrix_ty) =
                write_expression(&builder.expressions[matrix], module, builder)?;

            let ty = match *matrix_ty.borrow() {
                TypeInner::Matrix {
                    columns,
                    rows,
                    kind,
                    width,
                } => MaybeOwned::Owned(TypeInner::Matrix {
                    columns: rows,
                    rows: columns,
                    kind,
                    width,
                }),
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot apply transpose to {}",
                        matrix_expr
                    )))
                }
            };

            (Cow::Owned(format!("transpose({})", matrix_expr)), ty)
        }
        Expression::DotProduct(left, right) => {
            let (left_expr, left_ty) =
                write_expression(&builder.expressions[left], module, builder)?;
            let (right_expr, _) = write_expression(&builder.expressions[right], module, builder)?;

            let ty = match *left_ty.borrow() {
                TypeInner::Vector { kind, width, .. } => {
                    MaybeOwned::Owned(TypeInner::Scalar { kind, width })
                }
                _ => {
                    return Err(Error::Custom(format!(
                        "Cannot apply dot product to {}",
                        left_expr
                    )))
                }
            };

            (Cow::Owned(format!("dot({},{})", left_expr, right_expr)), ty)
        }
        Expression::CrossProduct(left, right) => {
            let (left_expr, left_ty) =
                write_expression(&builder.expressions[left], module, builder)?;
            let (right_expr, _) = write_expression(&builder.expressions[right], module, builder)?;

            (
                Cow::Owned(format!("cross({},{})", left_expr, right_expr)),
                left_ty,
            )
        }
        Expression::As {
            expr,
            kind,
            convert,
        } => {
            let (value_expr, value_ty) =
                write_expression(&builder.expressions[expr], module, builder)?;
            let (source_kind, ty_expr, out_ty) = match *value_ty.borrow() {
                TypeInner::Scalar { width, kind } => (
                    kind,
                    Cow::Borrowed(map_scalar(kind, width, builder.features)?.full),
                    MaybeOwned::Owned(TypeInner::Scalar { kind, width }),
                ),
                TypeInner::Vector { width, kind, size } => (
                    kind,
                    Cow::Owned(format!(
                        "{}vec",
                        map_scalar(kind, width, builder.features)?.prefix
                    )),
                    MaybeOwned::Owned(TypeInner::Vector { kind, width, size }),
                ),
                _ => return Err(Error::Custom(format!("Cannot convert {}", value_expr))),
            };
            let op = if convert {
                ty_expr
            } else {
                Cow::Borrowed(match (source_kind, kind) {
                    (ScalarKind::Float, ScalarKind::Sint) => "floatBitsToInt",
                    (ScalarKind::Float, ScalarKind::Uint) => "floatBitsToUInt",
                    (ScalarKind::Sint, ScalarKind::Float) => "intBitsToFloat",
                    (ScalarKind::Uint, ScalarKind::Float) => "uintBitsToFloat",
                    _ => {
                        return Err(Error::Custom(format!(
                            "Cannot bitcast {:?} to {:?}",
                            source_kind, kind
                        )))
                    }
                })
            };

            (Cow::Owned(format!("{}({})", op, value_expr)), out_ty)
        }
        Expression::Derivative { axis, expr } => {
            let (expr, ty) = write_expression(&builder.expressions[expr], module, builder)?;

            (
                Cow::Owned(format!(
                    "{}({})",
                    match axis {
                        DerivativeAxis::X => "dFdx",
                        DerivativeAxis::Y => "dFdy",
                        DerivativeAxis::Width => "fwidth",
                    },
                    expr
                )),
                ty,
            )
        }
        Expression::Call {
            ref origin,
            ref arguments,
        } => {
            let ty = match *origin {
                FunctionOrigin::Local(function) => module.functions[function]
                    .return_type
                    .map(|ty| module.borrow_type(ty))
                    .unwrap_or(MaybeOwned::Owned(
                        TypeInner::Sampler { comparison: false }, /*Dummy type*/
                    )),
                FunctionOrigin::External(_) => {
                    write_expression(&builder.expressions[arguments[0]], module, builder)?.1
                }
            };

            (
                Cow::Owned(format!(
                    "{}({})",
                    match *origin {
                        FunctionOrigin::External(ref name) => name,
                        FunctionOrigin::Local(handle) => builder.functions.get(&handle).unwrap(),
                    },
                    arguments
                        .iter()
                        .map::<Result<_, Error>, _>(|arg| Ok(write_expression(
                            &builder.expressions[*arg],
                            module,
                            builder
                        )?
                        .0))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                )),
                ty,
            )
        }
    })
}

fn write_constant(
    constant: &Constant,
    module: &Module,
    builder: &StatementBuilder<'_>,
    features: SupportedFeatures,
) -> Result<String, Error> {
    Ok(match constant.inner {
        ConstantInner::Sint(int) => int.to_string(),
        ConstantInner::Uint(int) => format!("{}u", int),
        ConstantInner::Float(float) => format!("{:?}", float),
        ConstantInner::Bool(boolean) => boolean.to_string(),
        ConstantInner::Composite(ref components) => format!(
            "{}({})",
            match module.types[constant.ty].inner {
                TypeInner::Vector { size, .. } => Cow::Owned(format!("vec{}", size as u8,)),
                TypeInner::Matrix { columns, rows, .. } =>
                    Cow::Owned(format!("mat{}x{}", columns as u8, rows as u8,)),
                TypeInner::Struct { .. } =>
                    Cow::<str>::Borrowed(builder.structs.get(&constant.ty).unwrap()),
                TypeInner::Array { .. } =>
                    write_type(constant.ty, &module.types, builder.structs, None, features)?,
                _ =>
                    return Err(Error::Custom(format!(
                        "Cannot build constant of type {}",
                        write_type(constant.ty, &module.types, builder.structs, None, features)?
                    ))),
            },
            components
                .iter()
                .map(|component| write_constant(
                    &module.constants[*component],
                    module,
                    builder,
                    features
                ))
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
        ),
    })
}

struct ScalarString<'a> {
    prefix: &'a str,
    full: &'a str,
}

fn map_scalar(
    kind: ScalarKind,
    width: crate::Bytes,
    features: SupportedFeatures,
) -> Result<ScalarString<'static>, Error> {
    Ok(match kind {
        ScalarKind::Sint => ScalarString {
            prefix: "i",
            full: "int",
        },
        ScalarKind::Uint => ScalarString {
            prefix: "u",
            full: "uint",
        },
        ScalarKind::Float => match width {
            4 => ScalarString {
                prefix: "",
                full: "float",
            },
            8 if features.contains(SupportedFeatures::DOUBLE_TYPE) => ScalarString {
                prefix: "d",
                full: "double",
            },
            _ => {
                return Err(Error::Custom(format!(
                    "Cannot build float of width {}",
                    width
                )))
            }
        },
        ScalarKind::Bool => ScalarString {
            prefix: "b",
            full: "bool",
        },
    })
}

fn write_type<'a>(
    ty: Handle<Type>,
    types: &Arena<Type>,
    structs: &'a FastHashMap<Handle<Type>, String>,
    block: Option<String>,
    features: SupportedFeatures,
) -> Result<Cow<'a, str>, Error> {
    Ok(match types[ty].inner {
        TypeInner::Scalar { kind, width } => Cow::Borrowed(map_scalar(kind, width, features)?.full),
        TypeInner::Vector { size, kind, width } => Cow::Owned(format!(
            "{}vec{}",
            map_scalar(kind, width, features)?.prefix,
            size as u8
        )),
        TypeInner::Matrix {
            columns,
            rows,
            kind,
            width,
        } => Cow::Owned(format!(
            "{}mat{}x{}",
            if (width == 4 && kind == ScalarKind::Float)
                || features.contains(SupportedFeatures::NON_FLOAT_MATRICES)
            {
                map_scalar(kind, width, features)?.prefix
            } else {
                return Err(Error::Custom(format!(
                    "Cannot build matrix of base type {:?}",
                    kind
                )));
            },
            columns as u8,
            rows as u8
        )),
        TypeInner::Pointer { base, .. } => write_type(base, types, structs, None, features)?,
        TypeInner::Array { base, size, .. } => Cow::Owned(format!(
            "{}[{}]",
            write_type(base, types, structs, None, features)?,
            write_array_size(size)?
        )),
        TypeInner::Struct { ref members } => {
            if let Some(name) = block {
                let mut out = String::new();
                writeln!(&mut out, "{} {{", name)?;

                for (idx, member) in members.iter().enumerate() {
                    writeln!(
                        &mut out,
                        "\t{} {};",
                        write_type(member.ty, types, structs, None, features)?,
                        member
                            .name
                            .clone()
                            .filter(|s| is_valid_ident(s))
                            .unwrap_or_else(|| format!("_{}", idx))
                    )?;
                }

                write!(&mut out, "}}")?;

                Cow::Owned(out)
            } else {
                Cow::Borrowed(structs.get(&ty).unwrap())
            }
        }
        _ => unreachable!(),
    })
}

fn write_image_type(
    kind: ScalarKind,
    dim: crate::ImageDimension,
    arrayed: bool,
    class: ImageClass,
    comparison: bool,
    features: SupportedFeatures,
) -> Result<String, Error> {
    if arrayed
        && dim != crate::ImageDimension::D2
        && !features.contains(SupportedFeatures::NON_2D_TEXTURE_ARRAYS)
    {
        return Err(Error::Custom(String::from(
            "Arrayed non 2d images aren't supported",
        )));
    }

    Ok(format!(
        "{}{}{}{}{}",
        match kind {
            ScalarKind::Sint => "i",
            ScalarKind::Uint => "u",
            ScalarKind::Float => "",
            ScalarKind::Bool =>
                return Err(Error::Custom(String::from(
                    "Cannot build image of booleans",
                ))),
        },
        match class {
            ImageClass::Storage(_) => "image",
            _ => "texture",
        },
        ImageDimension(dim),
        write_image_flags(arrayed, class, features)?,
        if comparison { "Shadow" } else { "" }
    ))
}

fn write_storage_class(
    class: StorageClass,
    features: SupportedFeatures,
) -> Result<&'static str, Error> {
    Ok(match class {
        StorageClass::Constant => "const ",
        StorageClass::Function => "",
        StorageClass::Input => "in ",
        StorageClass::Output => "out ",
        StorageClass::Private => "",
        StorageClass::StorageBuffer => {
            if features.contains(SupportedFeatures::BUFFER_STORAGE) {
                "buffer "
            } else {
                return Err(Error::Custom(String::from(
                    "buffer storage class isn't supported in glsl es",
                )));
            }
        }
        StorageClass::Uniform => "uniform ",
        StorageClass::WorkGroup => {
            if features.contains(SupportedFeatures::SHARED_STORAGE) {
                "shared "
            } else {
                return Err(Error::Custom(String::from(
                    "workgroup storage class isn't supported in glsl es",
                )));
            }
        }
    })
}

fn write_interpolation(interpolation: Interpolation) -> Result<&'static str, Error> {
    Ok(match interpolation {
        Interpolation::Perspective => "smooth",
        Interpolation::Linear => "noperspective",
        Interpolation::Flat => "flat",
        Interpolation::Centroid => "centroid",
        Interpolation::Sample => "sample",
        Interpolation::Patch => {
            return Err(Error::Custom(
                "patch interpolation qualifier not supported".to_string(),
            ))
        }
    })
}

fn write_array_size(size: ArraySize) -> Result<String, Error> {
    Ok(match size {
        ArraySize::Static(size) => size.to_string(),
        ArraySize::Dynamic => String::from(""),
    })
}

fn write_image_flags(
    arrayed: bool,
    class: ImageClass,
    features: SupportedFeatures,
) -> Result<String, Error> {
    let ms = match class {
        ImageClass::Multisampled => {
            if !features.contains(SupportedFeatures::MULTISAMPLED_TEXTURES) {
                return Err(Error::Custom(String::from(
                    "Multi sampled textures aren't supported",
                )));
            }
            if arrayed && !features.contains(SupportedFeatures::MULTISAMPLED_TEXTURE_ARRAYS) {
                return Err(Error::Custom(String::from(
                    "Multi sampled texture arrays aren't supported",
                )));
            }
            "MS"
        }
        _ => "",
    };

    let array = if arrayed { "Array" } else { "" };

    Ok(format!("{}{}", ms, array))
}

struct ImageDimension(crate::ImageDimension);
impl fmt::Display for ImageDimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self.0 {
                crate::ImageDimension::D1 => "1D",
                crate::ImageDimension::D2 => "2D",
                crate::ImageDimension::D3 => "3D",
                crate::ImageDimension::Cube => "Cube",
            }
        )
    }
}

fn write_struct(
    handle: Handle<Type>,
    members: &[StructMember],
    module: &Module,
    structs: &FastHashMap<Handle<Type>, String>,
    out: &mut impl Write,
    built_structs: &mut FastHashMap<Handle<Type>, ()>,
    features: SupportedFeatures,
) -> Result<(), Error> {
    if built_structs.get(&handle).is_some() {
        return Ok(());
    }

    let mut tmp = String::new();

    let name = structs.get(&handle).unwrap();

    writeln!(&mut tmp, "struct {} {{", name)?;
    for (idx, member) in members.iter().enumerate() {
        if let MemberOrigin::BuiltIn(_) = member.origin {
            continue;
        }

        if let TypeInner::Struct { ref members } = module.types[member.ty].inner {
            write_struct(
                member.ty,
                members,
                module,
                structs,
                out,
                built_structs,
                features,
            )?;
        }

        writeln!(
            &mut tmp,
            "\t{} {};",
            write_type(member.ty, &module.types, &structs, None, features)?,
            member
                .name
                .clone()
                .filter(|s| is_valid_ident(s))
                .unwrap_or_else(|| format!("_{}", idx))
        )?;
    }
    writeln!(&mut tmp, "}};")?;

    built_structs.insert(handle, ());

    writeln!(out, "{}", tmp)?;

    Ok(())
}

fn is_valid_ident(ident: &str) -> bool {
    ident.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && ident.contains(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && !ident.starts_with("gl_")
        && ident != "main"
}

fn builtin_to_glsl(builtin: BuiltIn) -> &'static str {
    match builtin {
        BuiltIn::Position => "gl_Position",
        BuiltIn::GlobalInvocationId => "gl_GlobalInvocationID",
        BuiltIn::BaseInstance => "gl_BaseInstance",
        BuiltIn::BaseVertex => "gl_BaseVertex",
        BuiltIn::ClipDistance => "gl_ClipDistance",
        BuiltIn::InstanceIndex => "gl_InstanceIndex",
        BuiltIn::VertexIndex => "gl_VertexIndex",
        BuiltIn::PointSize => "gl_PointSize",
        BuiltIn::FragCoord => "gl_FragCoord",
        BuiltIn::FrontFacing => "gl_FrontFacing",
        BuiltIn::SampleIndex => "gl_SampleID",
        BuiltIn::FragDepth => "gl_FragDepth",
        BuiltIn::LocalInvocationId => "gl_LocalInvocationID",
        BuiltIn::LocalInvocationIndex => "gl_LocalInvocationIndex",
        BuiltIn::WorkGroupId => "gl_WorkGroupID",
    }
}

fn write_format_glsl(format: StorageFormat) -> &'static str {
    match format {
        StorageFormat::R8Unorm => "r8",
        StorageFormat::R8Snorm => "r8_snorm",
        StorageFormat::R8Uint => "r8ui",
        StorageFormat::R8Sint => "r8i",
        StorageFormat::R16Uint => "r16ui",
        StorageFormat::R16Sint => "r16i",
        StorageFormat::R16Float => "r16f",
        StorageFormat::Rg8Unorm => "rg8",
        StorageFormat::Rg8Snorm => "rg8_snorm",
        StorageFormat::Rg8Uint => "rg8ui",
        StorageFormat::Rg8Sint => "rg8i",
        StorageFormat::R32Uint => "r32ui",
        StorageFormat::R32Sint => "r32i",
        StorageFormat::R32Float => "r32f",
        StorageFormat::Rg16Uint => "rg16ui",
        StorageFormat::Rg16Sint => "rg16i",
        StorageFormat::Rg16Float => "rg16f",
        StorageFormat::Rgba8Unorm => "rgba8ui",
        StorageFormat::Rgba8Snorm => "rgba8_snorm",
        StorageFormat::Rgba8Uint => "rgba8ui",
        StorageFormat::Rgba8Sint => "rgba8i",
        StorageFormat::Rgb10a2Unorm => "rgb10_a2ui",
        StorageFormat::Rg11b10Float => "r11f_g11f_b10f",
        StorageFormat::Rg32Uint => "rg32ui",
        StorageFormat::Rg32Sint => "rg32i",
        StorageFormat::Rg32Float => "rg32f",
        StorageFormat::Rgba16Uint => "rgba16ui",
        StorageFormat::Rgba16Sint => "rgba16i",
        StorageFormat::Rgba16Float => "rgba16f",
        StorageFormat::Rgba32Uint => "rgba32ui",
        StorageFormat::Rgba32Sint => "rgba32i",
        StorageFormat::Rgba32Float => "rgba32f",
    }
}

fn collect_texture_mapping(
    module: &Module,
    functions: &FastHashMap<Handle<Function>, String>,
) -> Result<Vec<TextureMapping>, Error> {
    fn collect_texture_mapping_expr(
        func: &Function,
        expr: Handle<Expression>,
        mappings: &mut Vec<TextureMapping>,
    ) -> Result<(), Error> {
        match func.expressions[expr] {
            Expression::Access { base, index } => {
                collect_texture_mapping_expr(func, base, mappings)?;
                collect_texture_mapping_expr(func, index, mappings)?;
            }
            Expression::AccessIndex { base, .. } => {
                collect_texture_mapping_expr(func, base, mappings)?
            }
            Expression::Compose { ref components, .. } => {
                for comp in components {
                    collect_texture_mapping_expr(func, *comp, mappings)?
                }
            }
            Expression::Load { pointer } => collect_texture_mapping_expr(func, pointer, mappings)?,
            Expression::ImageSample {
                image,
                sampler,
                coordinate,
                level,
                depth_ref,
            } => {
                let tex_handle = match func.expressions[image] {
                    Expression::GlobalVariable(global) => global,
                    _ => unreachable!(),
                };

                let sampler_handle = match func.expressions[sampler] {
                    Expression::GlobalVariable(global) => global,
                    _ => unreachable!(),
                };

                collect_texture_mapping_expr(func, coordinate, mappings)?;
                match level {
                    crate::SampleLevel::Exact(expr) | crate::SampleLevel::Bias(expr) => {
                        collect_texture_mapping_expr(func, expr, mappings)?
                    }
                    crate::SampleLevel::Auto => {}
                }
                if let Some(expr) = depth_ref {
                    collect_texture_mapping_expr(func, expr, mappings)?;
                }

                let mapping = mappings.iter().find(|map| map.texture == tex_handle);

                if mapping.map_or(false, |map| map.sampler != sampler_handle) {
                    return Err(Error::Custom(String::from(
                        "Cannot use texture with two different samplers",
                    )));
                }

                if mapping.is_none() {
                    mappings.push(TextureMapping {
                        texture: tex_handle,
                        sampler: sampler_handle,
                    });
                }
            }
            Expression::ImageLoad {
                coordinate, index, ..
            } => {
                collect_texture_mapping_expr(func, coordinate, mappings)?;
                collect_texture_mapping_expr(func, index, mappings)?;
            }
            Expression::Unary { expr, .. } => collect_texture_mapping_expr(func, expr, mappings)?,
            Expression::Binary { left, right, .. } => {
                collect_texture_mapping_expr(func, left, mappings)?;
                collect_texture_mapping_expr(func, right, mappings)?;
            }
            Expression::Intrinsic { argument, .. } => {
                collect_texture_mapping_expr(func, argument, mappings)?
            }
            Expression::Transpose(expr) => collect_texture_mapping_expr(func, expr, mappings)?,
            Expression::DotProduct(left, right) => {
                collect_texture_mapping_expr(func, left, mappings)?;
                collect_texture_mapping_expr(func, right, mappings)?;
            }
            Expression::CrossProduct(left, right) => {
                collect_texture_mapping_expr(func, left, mappings)?;
                collect_texture_mapping_expr(func, right, mappings)?;
            }
            Expression::As(expr, _) => collect_texture_mapping_expr(func, expr, mappings)?,
            Expression::Derivative { expr, .. } => {
                collect_texture_mapping_expr(func, expr, mappings)?
            }
            Expression::Call { ref arguments, .. } => {
                for arg in arguments {
                    collect_texture_mapping_expr(func, *arg, mappings)?
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn collect_texture_mapping_sta(
        func: &Function,
        sta: &Statement,
        mappings: &mut Vec<TextureMapping>,
    ) -> Result<(), Error> {
        match sta {
            Statement::Block(block) => {
                for sta in block {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
            }
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                collect_texture_mapping_expr(func, *condition, mappings)?;

                for sta in accept {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
                for sta in reject {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
            }
            Statement::Switch {
                selector,
                cases,
                default,
            } => {
                collect_texture_mapping_expr(func, *selector, mappings)?;

                for sta in cases.values().flat_map(|c| &c.0) {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
                for sta in default {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
            }
            Statement::Loop { body, continuing } => {
                for sta in body {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
                for sta in continuing {
                    collect_texture_mapping_sta(func, sta, mappings)?;
                }
            }
            Statement::Return { value } => {
                if let Some(handle) = value {
                    collect_texture_mapping_expr(func, *handle, mappings)?;
                }
            }
            Statement::Store { pointer, value } => {
                collect_texture_mapping_expr(func, *pointer, mappings)?;
                collect_texture_mapping_expr(func, *value, mappings)?;
            }
            _ => {}
        }

        Ok(())
    }

    let mut mappings = Vec::new();

    for function in functions.keys() {
        let func = &module.functions[*function];
        for sta in func.body.iter() {
            collect_texture_mapping_sta(func, sta, &mut mappings)?;
        }
    }

    Ok(mappings)
}
