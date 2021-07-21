use std::collections::HashSet;

use crate::Handle;

/// Collection of handles for unused items in a module
#[derive(Default, Clone, Debug)]
pub struct UnusedItems {
    pub types: HashSet<Handle<crate::Type>>,
    pub global_variables: HashSet<Handle<crate::GlobalVariable>>,
    pub constants: HashSet<Handle<crate::Constant>>,
    pub functions: HashSet<Handle<crate::Function>>,
}

/// Gets the list of unused items for a given module
///
/// You may optionally specify an entry point and this will detect all items not used by that
/// entrypoint
pub fn get_unused_items<'a>(
    module: &'a crate::Module,
    entry_point: Option<&'a crate::EntryPoint>,
) -> UnusedItems {
    // Lists of used items
    let mut used_global_variables = HashSet::with_capacity(module.global_variables.len());
    let mut used_types = HashSet::with_capacity(module.types.len());
    let mut used_constants = HashSet::with_capacity(module.constants.len());
    let mut used_functions = HashSet::with_capacity(module.functions.len());

    // Lists of unused items
    let mut unused = UnusedItems::default();

    // Get the entry point to use as a usage reference, or else just look through all entrypoints
    let entry_points = if let Some(entry_point) = entry_point {
        vec![entry_point]
    } else {
        module.entry_points.iter().collect::<Vec<_>>()
    };

    // For every entrypoint
    for entry_point in entry_points {
        let ctx = &mut CrawlContext {
            module,
            function: &entry_point.function,
            used_global_variables: &mut used_global_variables,
            used_types: &mut used_types,
            used_constants: &mut used_constants,
            used_functions: &mut used_functions,
        };

        // Add all argument types to the used types list
        for arg in &entry_point.function.arguments {
            crawl_type(arg.ty, &ctx.module.types, ctx.used_types);
        }

        // Crawl the function body and add any global variables and typesthat are used to the used list
        crawl_block(&entry_point.function.body, ctx);
    }

    // For every used global variable
    for global_variable in &used_global_variables {
        // Add it's type to the list of used types
        let var = &module.global_variables[*global_variable];
        crawl_type(var.ty, &module.types, &mut used_types);
    }

    // Any items that are not in the used list, add to the unused list
    for (handle, _) in module.types.iter() {
        if !used_types.contains(&handle) {
            unused.types.insert(handle);
        }
    }
    for (handle, _) in module.global_variables.iter() {
        if !used_global_variables.contains(&handle) {
            unused.global_variables.insert(handle);
        }
    }
    for (handle, _) in module.constants.iter() {
        if !used_constants.contains(&handle) {
            unused.constants.insert(handle);
        }
    }
    for (handle, _) in module.functions.iter() {
        if !used_functions.contains(&handle) {
            unused.functions.insert(handle);
        }
    }

    // Return the unused items
    unused
}

/// Context for module crawling functions
struct CrawlContext<'a> {
    module: &'a crate::Module,
    function: &'a crate::Function,
    used_global_variables: &'a mut HashSet<Handle<crate::GlobalVariable>>,
    used_types: &'a mut HashSet<Handle<crate::Type>>,
    used_constants: &'a mut HashSet<Handle<crate::Constant>>,
    used_functions: &'a mut HashSet<Handle<crate::Function>>,
}

/// Crawl a constant and register used items
fn crawl_constant(constant: Handle<crate::Constant>, ctx: &mut CrawlContext) {
    ctx.used_constants.insert(constant);
    if let crate::ConstantInner::Composite { ty, ref components } =
        ctx.module.constants[constant].inner
    {
        crawl_type(ty, &ctx.module.types, ctx.used_types);

        for component in components {
            crawl_constant(*component, ctx);
        }
    }
}

/// Crawl a type and register sub-types
fn crawl_type(
    ty: Handle<crate::Type>,
    types: &crate::Arena<crate::Type>,
    used_types: &mut HashSet<Handle<crate::Type>>,
) {
    used_types.insert(ty);

    // Crawl any sub-types
    match types[ty].inner {
        crate::TypeInner::Scalar { kind: _, width: _ } => (),
        crate::TypeInner::Vector {
            size: _,
            kind: _,
            width: _,
        } => (),
        crate::TypeInner::Matrix {
            columns: _,
            rows: _,
            width: _,
        } => (),
        crate::TypeInner::Pointer { base, class: _ } => {
            crawl_type(base, types, used_types);
        }
        crate::TypeInner::ValuePointer {
            size: _,
            kind: _,
            width: _,
            class: _,
        } => (),
        crate::TypeInner::Array {
            base,
            size: _,
            stride: _,
        } => {
            crawl_type(base, types, used_types);
        }
        crate::TypeInner::Struct {
            top_level: _,
            ref members,
            span: _,
        } => {
            for member in members {
                crawl_type(member.ty, types, used_types);
            }
        }
        crate::TypeInner::Image {
            dim: _,
            arrayed: _,
            class: _,
        } => (),
        crate::TypeInner::Sampler { comparison: _ } => (),
    }
}

/// Crawl an expression and register used items
fn crawl_expr(expression: Handle<crate::Expression>, ctx: &mut CrawlContext) {
    match ctx.function.expressions[expression] {
        crate::Expression::GlobalVariable(handle) => {
            ctx.used_global_variables.insert(handle);
        }
        crate::Expression::Access { base, index } => {
            crawl_expr(base, ctx);
            crawl_expr(index, ctx);
        }
        crate::Expression::AccessIndex { base, index: _ } => {
            crawl_expr(base, ctx);
        }
        crate::Expression::Constant(constant) => {
            crawl_constant(constant, ctx);
        }
        crate::Expression::Splat { size: _, value } => {
            crawl_expr(value, ctx);
        }
        crate::Expression::Swizzle {
            size: _,
            vector,
            pattern: _,
        } => {
            crawl_expr(vector, ctx);
        }
        crate::Expression::Compose { ty, ref components } => {
            crawl_type(ty, &ctx.module.types, ctx.used_types);
            for component in components {
                crawl_expr(*component, ctx);
            }
        }
        crate::Expression::FunctionArgument(_) => (),
        crate::Expression::LocalVariable(local) => {
            crawl_type(
                ctx.function.local_variables[local].ty,
                &ctx.module.types,
                ctx.used_types,
            );
        }
        crate::Expression::Load { pointer } => {
            crawl_expr(pointer, ctx);
        }
        crate::Expression::ImageSample {
            image,
            sampler,
            coordinate,
            array_index,
            offset: _,
            level: _,
            depth_ref,
        } => {
            for expr in [
                Some(image),
                Some(sampler),
                Some(coordinate),
                array_index,
                depth_ref,
            ]
            .iter()
            .flatten()
            {
                crawl_expr(*expr, ctx);
            }
        }
        crate::Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            index,
        } => {
            for expr in &[Some(image), Some(coordinate), array_index, index] {
                if let Some(expr) = *expr {
                    crawl_expr(expr, ctx);
                }
            }
        }
        crate::Expression::ImageQuery { image, query } => {
            crawl_expr(image, ctx);
            if let crate::ImageQuery::Size { level: Some(level) } = query {
                crawl_expr(level, ctx);
            }
        }
        crate::Expression::Unary { op: _, expr } => {
            crawl_expr(expr, ctx);
        }
        crate::Expression::Binary { op: _, left, right } => {
            crawl_expr(left, ctx);
            crawl_expr(right, ctx);
        }
        crate::Expression::Select {
            condition,
            accept,
            reject,
        } => {
            crawl_expr(condition, ctx);
            crawl_expr(accept, ctx);
            crawl_expr(reject, ctx);
        }
        crate::Expression::Derivative { axis: _, expr } => {
            crawl_expr(expr, ctx);
        }
        crate::Expression::Relational { fun: _, argument } => {
            crawl_expr(argument, ctx);
        }
        crate::Expression::Math {
            fun: _,
            arg,
            arg1,
            arg2,
        } => {
            crawl_expr(arg, ctx);
            if let Some(arg1) = arg1 {
                crawl_expr(arg1, ctx);
            }
            if let Some(arg2) = arg2 {
                crawl_expr(arg2, ctx);
            }
        }
        crate::Expression::As {
            expr,
            kind: _,
            convert: _,
        } => {
            crawl_expr(expr, ctx);
        }
        crate::Expression::Call(function) => {
            ctx.used_functions.insert(function);
            crawl_block(&ctx.module.functions[function].body, ctx);
        }
        crate::Expression::ArrayLength(expr) => {
            crawl_expr(expr, ctx);
        }
    }
}

/// Crawl a block and register used items
fn crawl_block(#[allow(clippy::ptr_arg)] block: &crate::Block, ctx: &mut CrawlContext) {
    for statement in block {
        match *statement {
            crate::Statement::Emit(ref range) => {
                for expr in range.clone() {
                    crawl_expr(expr, ctx)
                }
            }
            crate::Statement::Block(ref block) => crawl_block(block, ctx),
            crate::Statement::If {
                condition: _,
                ref accept,
                ref reject,
            } => {
                crawl_block(accept, ctx);
                crawl_block(reject, ctx);
            }
            crate::Statement::Switch {
                selector,
                ref cases,
                ref default,
            } => {
                crawl_expr(selector, ctx);
                for case in cases {
                    crawl_block(&case.body, ctx);
                }
                crawl_block(default, ctx);
            }
            crate::Statement::Loop {
                ref body,
                ref continuing,
            } => {
                crawl_block(body, ctx);
                crawl_block(continuing, ctx);
            }
            crate::Statement::Break => (),
            crate::Statement::Continue => (),
            crate::Statement::Return { value } => {
                if let Some(expression_handle) = value {
                    crawl_expr(expression_handle, ctx);
                }
            }
            crate::Statement::Kill => (),
            crate::Statement::Barrier(_) => (),
            crate::Statement::Store { pointer, value } => {
                crawl_expr(pointer, ctx);
                crawl_expr(value, ctx);
            }
            crate::Statement::ImageStore {
                image,
                coordinate,
                array_index,
                value,
            } => {
                crawl_expr(image, ctx);
                crawl_expr(coordinate, ctx);
                if let Some(index) = array_index {
                    crawl_expr(index, ctx);
                }
                crawl_expr(value, ctx);
            }
            crate::Statement::Call {
                function,
                ref arguments,
                result,
            } => {
                ctx.used_functions.insert(function);
                crawl_block(&ctx.module.functions[function].body, ctx);

                for arg in arguments {
                    crawl_expr(*arg, ctx);
                }

                if let Some(result) = result {
                    crawl_expr(result, ctx);
                }
            }
        }
    }
}
