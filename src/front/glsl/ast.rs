use super::{
    super::{Emitter, Typifier},
    constants::ConstantSolver,
    error::ErrorKind,
    SourceMetadata,
};
use crate::{
    proc::ResolveContext, Arena, BinaryOperator, Binding, Block, Constant, Expression, FastHashMap,
    Function, FunctionArgument, GlobalVariable, Handle, Interpolation, LocalVariable, Module,
    RelationalFunction, ResourceBinding, Sampling, ScalarKind, ShaderStage, Statement,
    StorageClass, Type, TypeInner, UnaryOperator,
};

#[derive(Debug, Clone, Copy)]
pub enum GlobalLookupKind {
    Variable(Handle<GlobalVariable>),
    BlockSelect(Handle<GlobalVariable>, u32),
}

#[derive(Debug, Clone, Copy)]
pub struct GlobalLookup {
    pub kind: GlobalLookupKind,
    pub entry_arg: Option<usize>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct FunctionSignature {
    pub name: String,
    pub parameters: Vec<Handle<Type>>,
}

#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    pub qualifiers: Vec<ParameterQualifier>,
    pub handle: Handle<Function>,
    /// Wheter this function was already defined or is just a prototype
    pub defined: bool,
    /// Wheter or not this function returns void (nothing)
    pub void: bool,
}

bitflags::bitflags! {
    pub struct EntryArgUse: u32 {
        const READ = 0x1;
        const WRITE = 0x2;
    }
}

bitflags::bitflags! {
    pub struct PrologueStage: u32 {
        const VERTEX = 0x1;
        const FRAGMENT = 0x2;
        const COMPUTE = 0x4;
    }
}

impl From<ShaderStage> for PrologueStage {
    fn from(stage: ShaderStage) -> Self {
        match stage {
            ShaderStage::Vertex => PrologueStage::VERTEX,
            ShaderStage::Fragment => PrologueStage::FRAGMENT,
            ShaderStage::Compute => PrologueStage::COMPUTE,
        }
    }
}

#[derive(Debug)]
pub struct EntryArg {
    pub binding: Binding,
    pub handle: Handle<GlobalVariable>,
    pub prologue: PrologueStage,
}

#[derive(Debug)]
pub struct Program<'a> {
    pub version: u16,
    pub profile: Profile,
    pub entry_points: &'a FastHashMap<String, ShaderStage>,

    pub workgroup_size: [u32; 3],
    pub early_fragment_tests: bool,

    pub lookup_function: FastHashMap<FunctionSignature, FunctionDeclaration>,
    pub lookup_type: FastHashMap<String, Handle<Type>>,

    pub global_variables: Vec<(String, GlobalLookup)>,
    pub constants: Vec<(String, Handle<Constant>)>,

    pub entry_args: Vec<EntryArg>,
    pub entries: Vec<(String, ShaderStage, Handle<Function>)>,
    // TODO: More efficient representation
    pub function_arg_use: Vec<Vec<EntryArgUse>>,

    pub module: Module,
}

impl<'a> Program<'a> {
    pub fn new(entry_points: &'a FastHashMap<String, ShaderStage>) -> Program<'a> {
        Program {
            version: 0,
            profile: Profile::Core,
            entry_points,

            workgroup_size: [1; 3],
            early_fragment_tests: false,

            lookup_function: FastHashMap::default(),
            lookup_type: FastHashMap::default(),
            global_variables: Vec::new(),
            constants: Vec::new(),

            entry_args: Vec::new(),
            entries: Vec::new(),
            function_arg_use: Vec::new(),

            module: Module::default(),
        }
    }

    pub fn resolve_type<'b>(
        &'b mut self,
        context: &'b mut Context,
        handle: Handle<Expression>,
        meta: SourceMetadata,
    ) -> Result<&'b TypeInner, ErrorKind> {
        let resolve_ctx = ResolveContext {
            constants: &self.module.constants,
            types: &self.module.types,
            global_vars: &self.module.global_variables,
            local_vars: &context.locals,
            functions: &self.module.functions,
            arguments: &context.arguments,
        };
        match context
            .typifier
            .grow(handle, &context.expressions, &resolve_ctx)
        {
            //TODO: better error report
            Err(error) => Err(ErrorKind::SemanticError(
                meta,
                format!("Can't resolve type: {:?}", error).into(),
            )),
            Ok(()) => Ok(context.typifier.get(handle, &self.module.types)),
        }
    }

    pub fn resolve_handle(
        &mut self,
        context: &mut Context,
        handle: Handle<Expression>,
        meta: SourceMetadata,
    ) -> Result<Handle<Type>, ErrorKind> {
        let resolve_ctx = ResolveContext {
            constants: &self.module.constants,
            types: &self.module.types,
            global_vars: &self.module.global_variables,
            local_vars: &context.locals,
            functions: &self.module.functions,
            arguments: &context.arguments,
        };
        match context
            .typifier
            .grow(handle, &context.expressions, &resolve_ctx)
        {
            //TODO: better error report
            Err(error) => Err(ErrorKind::SemanticError(
                meta,
                format!("Can't resolve type: {:?}", error).into(),
            )),
            Ok(()) => Ok(context.typifier.get_handle(handle, &mut self.module.types)),
        }
    }

    pub fn solve_constant(
        &mut self,
        ctx: &Context,
        root: Handle<Expression>,
        meta: SourceMetadata,
    ) -> Result<Handle<Constant>, ErrorKind> {
        let mut solver = ConstantSolver {
            types: &self.module.types,
            expressions: ctx.expressions,
            constants: &mut self.module.constants,
        };

        solver.solve(root).map_err(|e| (meta, e).into())
    }
}

#[derive(Debug, PartialEq)]
pub enum Profile {
    Core,
}

#[derive(Debug)]
pub struct Context<'function> {
    expressions: &'function mut Arena<Expression>,
    pub locals: &'function mut Arena<LocalVariable>,
    pub arguments: &'function mut Vec<FunctionArgument>,
    pub arg_use: Vec<EntryArgUse>,

    //TODO: Find less allocation heavy representation
    pub scopes: Vec<FastHashMap<String, VariableReference>>,
    pub lookup_global_var_exps: FastHashMap<String, VariableReference>,
    pub samplers: FastHashMap<Handle<Expression>, Handle<Expression>>,
    pub typifier: Typifier,

    pub hir_exprs: Arena<HirExpr>,
    emitter: Emitter,
}

impl<'function> Context<'function> {
    pub fn new(
        program: &mut Program,
        body: &mut Block,
        expressions: &'function mut Arena<Expression>,
        locals: &'function mut Arena<LocalVariable>,
        arguments: &'function mut Vec<FunctionArgument>,
    ) -> Self {
        let mut this = Context {
            expressions,
            locals,
            arguments,
            arg_use: vec![EntryArgUse::empty(); program.entry_args.len()],

            scopes: vec![FastHashMap::default()],
            lookup_global_var_exps: FastHashMap::with_capacity_and_hasher(
                program.constants.len() + program.global_variables.len(),
                Default::default(),
            ),
            typifier: Typifier::new(),
            samplers: FastHashMap::default(),

            hir_exprs: Arena::default(),
            emitter: Emitter::default(),
        };

        for &(ref name, handle) in program.constants.iter() {
            let expr = this.expressions.append(Expression::Constant(handle));
            let var = VariableReference {
                expr,
                load: None,
                mutable: false,
                entry_arg: None,
            };

            this.lookup_global_var_exps.insert(name.into(), var);
        }

        this.emit_start();

        for &(ref name, lookup) in program.global_variables.iter() {
            this.emit_flush(body);
            let GlobalLookup { kind, entry_arg } = lookup;
            let (expr, load) = match kind {
                GlobalLookupKind::Variable(v) => {
                    let res = (
                        this.expressions.append(Expression::GlobalVariable(v)),
                        program.module.global_variables[v].class != StorageClass::Handle,
                    );
                    this.emit_start();

                    res
                }
                GlobalLookupKind::BlockSelect(handle, index) => {
                    let base = this.expressions.append(Expression::GlobalVariable(handle));
                    this.emit_start();
                    let expr = this
                        .expressions
                        .append(Expression::AccessIndex { base, index });

                    (expr, true)
                }
            };

            let var = VariableReference {
                expr,
                load: if load {
                    Some(this.add_expression(Expression::Load { pointer: expr }, body))
                } else {
                    None
                },
                // TODO: respect constant qualifier
                mutable: true,
                entry_arg,
            };

            this.lookup_global_var_exps.insert(name.into(), var);
        }

        this
    }

    pub fn emit_start(&mut self) {
        self.emitter.start(&self.expressions)
    }

    pub fn emit_flush(&mut self, body: &mut Block) {
        body.extend(self.emitter.finish(&self.expressions))
    }

    pub fn add_expression(&mut self, expr: Expression, body: &mut Block) -> Handle<Expression> {
        if expr.needs_pre_emit() {
            self.emit_flush(body);
            let handle = self.expressions.append(expr);
            self.emit_start();
            handle
        } else {
            self.expressions.append(expr)
        }
    }

    pub fn lookup_local_var(&self, name: &str) -> Option<VariableReference> {
        for scope in self.scopes.iter().rev() {
            if let Some(var) = scope.get(name) {
                return Some(var.clone());
            }
        }
        None
    }

    pub fn lookup_global_var(&mut self, name: &str) -> Option<VariableReference> {
        self.lookup_global_var_exps.get(name).cloned()
    }

    #[cfg(feature = "glsl-validate")]
    pub fn lookup_local_var_current_scope(&self, name: &str) -> Option<VariableReference> {
        if let Some(current) = self.scopes.last() {
            current.get(name).cloned()
        } else {
            None
        }
    }

    /// Add variable to current scope
    pub fn add_local_var(&mut self, name: String, expr: Handle<Expression>, mutable: bool) {
        if let Some(current) = self.scopes.last_mut() {
            let load = self.expressions.append(Expression::Load { pointer: expr });

            (*current).insert(
                name,
                VariableReference {
                    expr,
                    load: Some(load),
                    mutable,
                    entry_arg: None,
                },
            );
        }
    }

    /// Add function argument to current scope
    pub fn add_function_arg(
        &mut self,
        program: &mut Program,
        sig: &mut FunctionSignature,
        body: &mut Block,
        name: Option<String>,
        ty: Handle<Type>,
        qualifier: ParameterQualifier,
    ) {
        let index = self.arguments.len();
        let mut arg = FunctionArgument {
            name: name.clone(),
            ty,
            binding: None,
        };
        sig.parameters.push(ty);

        if qualifier.is_lhs() {
            arg.ty = program.module.types.fetch_or_append(Type {
                name: None,
                inner: TypeInner::Pointer {
                    base: arg.ty,
                    class: StorageClass::Function,
                },
            })
        }

        self.arguments.push(arg);

        if let Some(name) = name {
            let expr = self.add_expression(Expression::FunctionArgument(index as u32), body);
            let mutable = qualifier != ParameterQualifier::Const;
            let load = if qualifier.is_lhs() {
                Some(self.add_expression(Expression::Load { pointer: expr }, body))
            } else {
                None
            };

            if let Some(current) = self.scopes.last_mut() {
                (*current).insert(
                    name,
                    VariableReference {
                        expr,
                        load,
                        mutable,
                        entry_arg: None,
                    },
                );
            }
        }
    }

    /// Add new empty scope
    pub fn push_scope(&mut self) {
        self.scopes.push(FastHashMap::default());
    }

    pub fn remove_current_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn lower_expect(
        &mut self,
        program: &mut Program,
        expr: Handle<HirExpr>,
        lhs: bool,
        body: &mut Block,
    ) -> Result<(Handle<Expression>, SourceMetadata), ErrorKind> {
        let (maybe_expr, meta) = self.lower(program, expr, lhs, body)?;

        let expr = match maybe_expr {
            Some(e) => e,
            None => {
                return Err(ErrorKind::SemanticError(
                    meta,
                    "Expression returns void".into(),
                ))
            }
        };

        Ok((expr, meta))
    }

    pub fn lower(
        &mut self,
        program: &mut Program,
        expr: Handle<HirExpr>,
        lhs: bool,
        body: &mut Block,
    ) -> Result<(Option<Handle<Expression>>, SourceMetadata), ErrorKind> {
        let HirExpr { kind, meta } = self.hir_exprs[expr].clone();

        let handle = match kind {
            HirExprKind::Access { base, index } => {
                let base = self.lower_expect(program, base, lhs, body)?.0;
                let index = self.lower_expect(program, index, false, body)?.0;

                self.add_expression(Expression::Access { base, index }, body)
            }
            HirExprKind::Select { base, field } => {
                let base = self.lower_expect(program, base, lhs, body)?.0;

                program.field_selection(self, body, base, &field, meta)?
            }
            HirExprKind::Constant(constant) if !lhs => {
                self.add_expression(Expression::Constant(constant), body)
            }
            HirExprKind::Binary { left, op, right } if !lhs => {
                let (mut left, left_meta) = self.lower_expect(program, left, false, body)?;
                let (mut right, right_meta) = self.lower_expect(program, right, false, body)?;

                self.binary_implicit_conversion(
                    program, &mut left, left_meta, &mut right, right_meta,
                )?;

                if let BinaryOperator::Equal | BinaryOperator::NotEqual = op {
                    let equals = op == BinaryOperator::Equal;
                    let (left_is_vector, left_dims) =
                        match *program.resolve_type(self, left, left_meta)? {
                            crate::TypeInner::Vector { .. } => (true, 1),
                            crate::TypeInner::Matrix { .. } => (false, 2),
                            _ => (false, 0),
                        };

                    let (right_is_vector, right_dims) =
                        match *program.resolve_type(self, right, right_meta)? {
                            crate::TypeInner::Vector { .. } => (true, 1),
                            crate::TypeInner::Matrix { .. } => (false, 2),
                            _ => (false, 0),
                        };

                    let (op, fun) = match equals {
                        true => (BinaryOperator::Equal, RelationalFunction::All),
                        false => (BinaryOperator::NotEqual, RelationalFunction::Any),
                    };

                    let argument = self
                        .expressions
                        .append(Expression::Binary { op, left, right });

                    if left_dims != right_dims {
                        return Err(ErrorKind::SemanticError(meta, "Cannot compare".into()));
                    } else if left_is_vector && right_is_vector {
                        self.add_expression(Expression::Relational { fun, argument }, body)
                    } else {
                        argument
                    }
                } else {
                    self.add_expression(Expression::Binary { left, op, right }, body)
                }
            }
            HirExprKind::Unary { op, expr } if !lhs => {
                let expr = self.lower_expect(program, expr, false, body)?.0;

                self.add_expression(Expression::Unary { op, expr }, body)
            }
            HirExprKind::Variable(var) => {
                if lhs {
                    if !var.mutable {
                        return Err(ErrorKind::SemanticError(
                            meta,
                            "Variable cannot be used in LHS position".into(),
                        ));
                    }

                    if let Some(idx) = var.entry_arg {
                        self.arg_use[idx] |= EntryArgUse::WRITE
                    }

                    var.expr
                } else {
                    if let Some(idx) = var.entry_arg {
                        self.arg_use[idx] |= EntryArgUse::READ
                    }

                    var.load.unwrap_or(var.expr)
                }
            }
            HirExprKind::Call(call) if !lhs => {
                let maybe_expr = program.function_call(self, body, call.kind, &call.args, meta)?;
                return Ok((maybe_expr, meta));
            }
            HirExprKind::Conditional {
                condition,
                accept,
                reject,
            } if !lhs => {
                let condition = self.lower_expect(program, condition, false, body)?.0;
                let (mut accept, accept_meta) = self.lower_expect(program, accept, false, body)?;
                let (mut reject, reject_meta) = self.lower_expect(program, reject, false, body)?;

                self.binary_implicit_conversion(
                    program,
                    &mut accept,
                    accept_meta,
                    &mut reject,
                    reject_meta,
                )?;

                self.add_expression(
                    Expression::Select {
                        condition,
                        accept,
                        reject,
                    },
                    body,
                )
            }
            HirExprKind::Assign { tgt, value } if !lhs => {
                let (pointer, ptr_meta) = self.lower_expect(program, tgt, true, body)?;
                let (mut value, value_meta) = self.lower_expect(program, value, false, body)?;

                let ptr_kind = match *program.resolve_type(self, pointer, ptr_meta)? {
                    TypeInner::Pointer { base, .. } => {
                        program.module.types[base].inner.scalar_kind()
                    }
                    ref ty => ty.scalar_kind(),
                };

                if let Some(kind) = ptr_kind {
                    self.implicit_conversion(program, &mut value, value_meta, kind)?;
                }

                self.emit_flush(body);
                self.emit_start();

                body.push(Statement::Store { pointer, value });

                value
            }
            HirExprKind::IncDec {
                increment,
                postfix,
                expr,
            } => {
                let op = match increment {
                    true => BinaryOperator::Add,
                    false => BinaryOperator::Subtract,
                };

                let pointer = self.lower_expect(program, expr, true, body)?.0;
                let left = self.add_expression(Expression::Load { pointer }, body);

                let uint = if let Some(kind) = program.resolve_type(self, left, meta)?.scalar_kind()
                {
                    match kind {
                        ScalarKind::Sint => false,
                        ScalarKind::Uint => true,
                        _ => {
                            return Err(ErrorKind::SemanticError(
                                meta,
                                "Increment/decrement operations must operate in integers".into(),
                            ))
                        }
                    }
                } else {
                    return Err(ErrorKind::SemanticError(
                        meta,
                        "Increment/decrement operations must operate in integers".into(),
                    ));
                };

                let one = program.module.constants.append(Constant {
                    name: None,
                    specialization: None,
                    inner: crate::ConstantInner::Scalar {
                        width: 4,
                        value: match uint {
                            true => crate::ScalarValue::Uint(1),
                            false => crate::ScalarValue::Sint(1),
                        },
                    },
                });
                let right = self.add_expression(Expression::Constant(one), body);

                let value = self.add_expression(Expression::Binary { op, left, right }, body);

                if postfix {
                    let local = self.locals.append(LocalVariable {
                        name: None,
                        ty: program.module.types.fetch_or_append(Type {
                            name: None,
                            inner: TypeInner::Scalar {
                                kind: match uint {
                                    true => ScalarKind::Uint,
                                    false => ScalarKind::Sint,
                                },
                                width: 4,
                            },
                        }),
                        init: None,
                    });

                    let expr = self.add_expression(Expression::LocalVariable(local), body);
                    let load = self.add_expression(Expression::Load { pointer: expr }, body);

                    self.emit_flush(body);
                    self.emit_start();

                    body.push(Statement::Store {
                        pointer: expr,
                        value: left,
                    });

                    self.emit_flush(body);
                    self.emit_start();

                    body.push(Statement::Store { pointer, value });

                    load
                } else {
                    self.emit_flush(body);
                    self.emit_start();

                    body.push(Statement::Store { pointer, value });

                    left
                }
            }
            _ => {
                return Err(ErrorKind::SemanticError(
                    meta,
                    format!("{:?} cannot be in the left hand side", self.hir_exprs[expr]).into(),
                ))
            }
        };

        Ok((Some(handle), meta))
    }

    pub fn expr_scalar_kind(
        &mut self,
        program: &mut Program,
        expr: Handle<Expression>,
        meta: SourceMetadata,
    ) -> Result<Option<ScalarKind>, ErrorKind> {
        Ok(program.resolve_type(self, expr, meta)?.scalar_kind())
    }

    pub fn expr_power(
        &mut self,
        program: &mut Program,
        expr: Handle<Expression>,
        meta: SourceMetadata,
    ) -> Result<Option<u32>, ErrorKind> {
        Ok(self
            .expr_scalar_kind(program, expr, meta)?
            .and_then(type_power))
    }

    pub fn implicit_conversion(
        &mut self,
        program: &mut Program,
        expr: &mut Handle<Expression>,
        meta: SourceMetadata,
        kind: ScalarKind,
    ) -> Result<(), ErrorKind> {
        if let (Some(tgt_power), Some(expr_power)) =
            (type_power(kind), self.expr_power(program, *expr, meta)?)
        {
            if tgt_power > expr_power {
                *expr = self.expressions.append(Expression::As {
                    expr: *expr,
                    kind,
                    convert: None,
                })
            }
        }

        Ok(())
    }

    pub fn binary_implicit_conversion(
        &mut self,
        program: &mut Program,
        left: &mut Handle<Expression>,
        left_meta: SourceMetadata,
        right: &mut Handle<Expression>,
        right_meta: SourceMetadata,
    ) -> Result<(), ErrorKind> {
        let left_kind = self.expr_scalar_kind(program, *left, left_meta)?;
        let right_kind = self.expr_scalar_kind(program, *right, right_meta)?;

        if let (Some((left_power, left_kind)), Some((right_power, right_kind))) = (
            left_kind.and_then(|kind| Some((type_power(kind)?, kind))),
            right_kind.and_then(|kind| Some((type_power(kind)?, kind))),
        ) {
            match left_power.cmp(&right_power) {
                std::cmp::Ordering::Less => {
                    *left = self.expressions.append(Expression::As {
                        expr: *left,
                        kind: right_kind,
                        convert: None,
                    })
                }
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Greater => {
                    *right = self.expressions.append(Expression::As {
                        expr: *right,
                        kind: left_kind,
                        convert: None,
                    })
                }
            }
        }

        Ok(())
    }
}

fn type_power(kind: ScalarKind) -> Option<u32> {
    Some(match kind {
        ScalarKind::Sint => 0,
        ScalarKind::Uint => 1,
        ScalarKind::Float => 2,
        ScalarKind::Bool => return None,
    })
}

#[derive(Debug, Clone)]
pub struct VariableReference {
    pub expr: Handle<Expression>,
    pub load: Option<Handle<Expression>>,
    pub mutable: bool,
    pub entry_arg: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub meta: SourceMetadata,
}

#[derive(Debug, Clone)]
pub enum HirExprKind {
    Access {
        base: Handle<HirExpr>,
        index: Handle<HirExpr>,
    },
    Select {
        base: Handle<HirExpr>,
        field: String,
    },
    Constant(Handle<Constant>),
    Binary {
        left: Handle<HirExpr>,
        op: BinaryOperator,
        right: Handle<HirExpr>,
    },
    Unary {
        op: UnaryOperator,
        expr: Handle<HirExpr>,
    },
    Variable(VariableReference),
    Call(FunctionCall),
    Conditional {
        condition: Handle<HirExpr>,
        accept: Handle<HirExpr>,
        reject: Handle<HirExpr>,
    },
    Assign {
        tgt: Handle<HirExpr>,
        value: Handle<HirExpr>,
    },
    IncDec {
        increment: bool,
        postfix: bool,
        expr: Handle<HirExpr>,
    },
}

#[derive(Debug)]
pub enum TypeQualifier {
    StorageQualifier(StorageQualifier),
    Interpolation(Interpolation),
    ResourceBinding(ResourceBinding),
    Location(u32),
    WorkGroupSize(usize, u32),
    Sampling(Sampling),
    Layout(StructLayout),
    EarlyFragmentTests,
}

#[derive(Debug, Clone)]
pub enum FunctionCallKind {
    TypeConstructor(Handle<Type>),
    Function(String),
}

#[derive(Debug, Clone)]
pub struct FunctionCall {
    pub kind: FunctionCallKind,
    pub args: Vec<Handle<HirExpr>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StorageQualifier {
    StorageClass(StorageClass),
    Input,
    Output,
    Const,
}

#[derive(Debug, Clone, Copy)]
pub enum StructLayout {
    Std140,
    Std430,
}

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ParameterQualifier {
    In,
    Out,
    InOut,
    Const,
}

impl ParameterQualifier {
    /// Returns true if the argument should be passed as a lhs expression
    pub fn is_lhs(&self) -> bool {
        match *self {
            ParameterQualifier::Out | ParameterQualifier::InOut => true,
            _ => false,
        }
    }
}
