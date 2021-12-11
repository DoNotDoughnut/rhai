use crate::{Engine, AST};
use serde::{Deserialize, Serialize};
#[cfg(feature = "no_std")]
use std::prelude::v1::*;
use std::{cmp::Ordering, collections::BTreeMap};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FnType {
    Script,
    Native,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FnNamespace {
    Global,
    Internal,
}

impl From<crate::FnNamespace> for FnNamespace {
    fn from(value: crate::FnNamespace) -> Self {
        match value {
            crate::FnNamespace::Global => Self::Global,
            crate::FnNamespace::Internal => Self::Internal,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum FnAccess {
    Public,
    Private,
}

impl From<crate::FnAccess> for FnAccess {
    fn from(value: crate::FnAccess) -> Self {
        match value {
            crate::FnAccess::Public => Self::Public,
            crate::FnAccess::Private => Self::Private,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FnParam {
    pub name: Box<str>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub typ: Option<Box<str>>,
}

impl PartialOrd for FnParam {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(match self.name.partial_cmp(&other.name).expect("succeed") {
            Ordering::Less => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
            Ordering::Equal => match (self.typ.is_none(), other.typ.is_none()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self
                    .typ
                    .as_ref()
                    .expect("`Some`")
                    .partial_cmp(other.typ.as_ref().expect("`Some`"))
                    .expect("succeed"),
            },
        })
    }
}

impl Ord for FnParam {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.name.cmp(&other.name) {
            Ordering::Equal => self.typ.cmp(&other.typ),
            cmp => cmp,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FnMetadata {
    pub namespace: FnNamespace,
    pub access: FnAccess,
    pub name: String,
    #[serde(rename = "type")]
    pub typ: FnType,
    pub num_params: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<FnParam>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_type: Option<Box<str>>,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doc_comments: Vec<Box<str>>,
}

impl PartialOrd for FnMetadata {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FnMetadata {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.name.cmp(&other.name) {
            Ordering::Equal => match self.num_params.cmp(&other.num_params) {
                Ordering::Equal => self.params.cmp(&other.params),
                cmp => cmp,
            },
            cmp => cmp,
        }
    }
}

impl From<&crate::module::FuncInfo> for FnMetadata {
    fn from(info: &crate::module::FuncInfo) -> Self {
        Self {
            namespace: info.namespace.into(),
            access: info.access.into(),
            name: info.name.to_string(),
            typ: if info.func.is_script() {
                FnType::Script
            } else {
                FnType::Native
            },
            num_params: info.params,
            params: info
                .param_names
                .iter()
                .take(info.params)
                .map(|s| {
                    let mut seg = s.splitn(2, ':');
                    let name = seg
                        .next()
                        .map(|s| s.trim().into())
                        .unwrap_or_else(|| "_".into());
                    let typ = seg.next().map(|s| s.trim().into());
                    FnParam { name, typ }
                })
                .collect(),
            return_type: info
                .param_names
                .last()
                .map(|s| s.as_str().into())
                .or_else(|| Some("()".into())),
            signature: info.gen_signature(),
            doc_comments: if info.func.is_script() {
                #[cfg(feature = "no_function")]
                {
                    unreachable!("scripted functions should not exist under no_function")
                }
                #[cfg(not(feature = "no_function"))]
                {
                    info.func
                        .get_script_fn_def()
                        .expect("scripted function")
                        .comments
                        .as_ref()
                        .map_or_else(|| Vec::new(), |v| v.to_vec())
                }
            } else {
                Vec::new()
            },
        }
    }
}

#[cfg(not(feature = "no_function"))]
impl From<crate::ast::ScriptFnMetadata<'_>> for FnMetadata {
    fn from(info: crate::ast::ScriptFnMetadata) -> Self {
        Self {
            namespace: FnNamespace::Global,
            access: info.access.into(),
            name: info.name.to_string(),
            typ: FnType::Script,
            num_params: info.params.len(),
            params: info
                .params
                .iter()
                .map(|&s| FnParam {
                    name: s.into(),
                    typ: Some("Dynamic".into()),
                })
                .collect(),
            return_type: Some("Dynamic".into()),
            signature: info.to_string(),
            doc_comments: info.comments.iter().map(|&s| s.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModuleMetadata {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, Self>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FnMetadata>,
}

impl ModuleMetadata {
    #[inline(always)]
    pub fn new() -> Self {
        Self {
            modules: BTreeMap::new(),
            functions: Vec::new(),
        }
    }
}

impl From<&crate::Module> for ModuleMetadata {
    fn from(module: &crate::Module) -> Self {
        let mut functions: Vec<_> = module.iter_fn().map(|f| f.into()).collect();
        functions.sort();

        Self {
            modules: module
                .iter_sub_modules()
                .map(|(name, m)| (name.to_string(), m.as_ref().into()))
                .collect(),
            functions,
        }
    }
}

#[cfg(feature = "metadata")]
impl Engine {
    /// _(metadata)_ Generate a list of all functions (including those defined in an
    /// [`AST`][crate::AST]) in JSON format.
    /// Exported under the `metadata` feature only.
    ///
    /// Functions from the following sources are included:
    /// 1) Functions defined in an [`AST`][crate::AST]
    /// 2) Functions registered into the global namespace
    /// 3) Functions in static modules
    /// 4) Functions in global modules (optional)
    pub fn gen_fn_metadata_with_ast_to_json(
        &self,
        ast: &AST,
        include_global: bool,
    ) -> serde_json::Result<String> {
        let _ast = ast;
        let mut global = ModuleMetadata::new();

        if include_global {
            self.global_modules
                .iter()
                .take(self.global_modules.len() - 1)
                .flat_map(|m| m.iter_fn())
                .for_each(|f| global.functions.push(f.into()));
        }

        self.global_sub_modules.iter().for_each(|(name, m)| {
            global.modules.insert(name.to_string(), m.as_ref().into());
        });

        self.global_namespace()
            .iter_fn()
            .for_each(|f| global.functions.push(f.into()));

        #[cfg(not(feature = "no_function"))]
        _ast.iter_functions()
            .for_each(|f| global.functions.push(f.into()));

        global.functions.sort();

        serde_json::to_string_pretty(&global)
    }

    /// Generate a list of all functions in JSON format.
    /// Exported under the `metadata` feature only.
    ///
    /// Functions from the following sources are included:
    /// 1) Functions registered into the global namespace
    /// 2) Functions in static modules
    /// 3) Functions in global modules (optional)
    #[inline(always)]
    pub fn gen_fn_metadata_to_json(&self, include_global: bool) -> serde_json::Result<String> {
        self.gen_fn_metadata_with_ast_to_json(&AST::empty(), include_global)
    }
}
