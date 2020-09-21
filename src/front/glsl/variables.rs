use crate::{
    Binding, BuiltIn, Expression, GlobalVariable, Handle, ScalarKind, ShaderStage, StorageAccess,
    StorageClass, Type, TypeInner, VectorSize,
};

use super::ast::*;
use super::error::ErrorKind;

impl Program {
    fn lookup_global_struct_member(&mut self, name: &str) -> Option<Handle<Expression>> {
        let global_struct_member = self
            .module
            .global_variables
            .iter()
            // find globals without name
            .filter(|(_, var)| var.name.is_none())
            // find those of struct type and map to their members
            .filter_map(|(h, var)| {
                let ty = &self.module.types[var.ty];
                if let TypeInner::Struct { members } = &ty.inner {
                    Some((h, members))
                } else {
                    None
                }
            })
            // find member matching name
            .find_map(|(h, members)| {
                members
                    .iter()
                    .position(|m| m.name == Some(name.into()))
                    .map(|idx| (h, idx))
            });
        global_struct_member.map(|(h, idx)| {
            let base = self
                .context
                .expressions
                .append(Expression::GlobalVariable(h));
            self.context.expressions.append(Expression::AccessIndex {
                base,
                index: idx as u32,
            })
        })
    }

    pub fn lookup_variable(&mut self, name: &str) -> Result<Option<Handle<Expression>>, ErrorKind> {
        let mut expression: Option<Handle<Expression>> = None;
        match name {
            "gl_Position" => {
                #[cfg(feature = "glsl-validate")]
                match self.shader_stage {
                    ShaderStage::Vertex | ShaderStage::Fragment { .. } => {}
                    _ => {
                        return Err(ErrorKind::VariableNotAvailable(name.into()));
                    }
                };
                let h = self
                    .module
                    .global_variables
                    .fetch_or_append(GlobalVariable {
                        name: Some(name.into()),
                        class: if self.shader_stage == ShaderStage::Vertex {
                            StorageClass::Output
                        } else {
                            StorageClass::Input
                        },
                        binding: Some(Binding::BuiltIn(BuiltIn::Position)),
                        ty: self.module.types.fetch_or_append(Type {
                            name: None,
                            inner: TypeInner::Vector {
                                size: VectorSize::Quad,
                                kind: ScalarKind::Float,
                                width: 4,
                            },
                        }),
                        interpolation: None,
                        storage_access: StorageAccess::empty(),
                    });
                self.lookup_global_variables.insert(name.into(), h);
                let exp = self
                    .context
                    .expressions
                    .append(Expression::GlobalVariable(h));
                self.context.lookup_global_var_exps.insert(name.into(), exp);

                expression = Some(exp);
            }
            "gl_VertexIndex" => {
                #[cfg(feature = "glsl-validate")]
                match self.shader_stage {
                    ShaderStage::Vertex => {}
                    _ => {
                        return Err(ErrorKind::VariableNotAvailable(name.into()));
                    }
                };
                let h = self
                    .module
                    .global_variables
                    .fetch_or_append(GlobalVariable {
                        name: Some(name.into()),
                        class: StorageClass::Input,
                        binding: Some(Binding::BuiltIn(BuiltIn::VertexIndex)),
                        ty: self.module.types.fetch_or_append(Type {
                            name: None,
                            inner: TypeInner::Scalar {
                                kind: ScalarKind::Uint,
                                width: 4,
                            },
                        }),
                        interpolation: None,
                        storage_access: StorageAccess::empty(),
                    });
                self.lookup_global_variables.insert(name.into(), h);
                let exp = self
                    .context
                    .expressions
                    .append(Expression::GlobalVariable(h));
                self.context.lookup_global_var_exps.insert(name.into(), exp);

                expression = Some(exp);
            }
            _ => {}
        }

        if let Some(expression) = expression {
            Ok(Some(expression))
        } else if let Some(local_var) = self.context.lookup_local_var(name) {
            Ok(Some(local_var))
        } else if let Some(global_var) = self.context.lookup_global_var_exps.get(name) {
            Ok(Some(*global_var))
        } else if let Some(global_member) = self.lookup_global_struct_member(name) {
            Ok(Some(global_member))
        } else {
            Ok(None)
        }
    }
}
