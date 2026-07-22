//! Derive macro for `ConfigDiff`.
//!
//! 为 `Vec<Entry>` 包装类型自动实现 `bevy_assets_hmr::ConfigDiff`，
//! 基于 entry 的 id 字段做 added/removed/modified diff。
//!
//! # 用法
//!
//! ```ignore
//! use bevy::prelude::*;
//! use bevy::reflect::TypePath;
//! use serde::{Deserialize, Serialize};
//! use bevy_assets_hmr::ConfigDiff;
//!
//! #[derive(
//!     Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default,
//!     ConfigDiff,
//! )]
//! #[config_diff(field = "texts", id = "id")]
//! pub struct TextDatabase {
//!     pub texts: Vec<TextEntry>,
//! }
//!
//! #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
//! pub struct TextEntry {
//!     pub id: String,
//!     pub value: String,
//! }
//! ```
//!
//! # 约束
//!
//! - **Struct**: 数据库类型必须实现 `PartialEq`（检测 modified 条目）
//! - **Enum**: 枚举类型必须实现 `PartialEq`；diff 用整体比较，变了则将类型名
//!   作为 modified id。枚举**不支持** `#[config_diff(...)]` 属性（会编译报错）
//! - entry 的 id 字段类型默认是 `String`，可通过 `id_type` 指定为其他类型
//!  （如 `u32`、`uuid::Uuid` 等 `Eq + Hash + Clone` 类型）
//! - `field` 指定 `Vec<Entry>` 字段名；省略时自动找第一个 `Vec<_>` 字段
//! - `id` 指定 entry 的 id 字段名；省略时默认 `"id"`
//! - `id_type` 指定 id 字段类型；省略时默认 `"String"`
//!
//! # 非 String id 示例
//!
//! ```ignore
//! #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
//! #[config_diff(field = "entries", id = "id", id_type = "u32")]
//! pub struct MonsterDatabase {
//!     pub entries: Vec<MonsterEntry>,
//! }
//!
//! #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
//! pub struct MonsterEntry {
//!     pub id: u32,
//!     pub name: String,
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Ident, LitStr, Type};
use syn::spanned::Spanned;

/// Helper attribute: `#[config_diff(field = "texts", id = "id")]`
struct ConfigDiffAttr {
    field: Option<Ident>,
    id: Option<Ident>,
    /// The type of the id field, e.g. `String`, `u32`, `Uuid`.
    /// Defaults to `String` when not specified.
    id_type: Option<Type>,
}

impl syn::parse::Parse for ConfigDiffAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut field = None;
        let mut id = None;
        let mut id_type = None;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;
            let value: LitStr = input.parse()?;
            match key.to_string().as_str() {
                "field" => field = Some(value.parse()?),
                "id" => id = Some(value.parse()?),
                "id_type" => id_type = Some(value.parse()?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown config_diff option `{}`; expected `field`, `id`, or `id_type`",
                            other
                        ),
                    ))
                }
            }
            let _ = input.parse::<syn::Token![,]>();
        }
        Ok(Self { field, id, id_type })
    }
}

#[proc_macro_derive(ConfigDiff, attributes(config_diff))]
pub fn derive_config_diff(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let crate_path = quote! { bevy_assets_hmr };
    let db_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let expanded: TokenStream2 = match &input.data {
        syn::Data::Struct(_) => {
            // Existing Vec<Entry> field-based diff.
            let field = match resolve_field(&input) {
                Ok(f) => f,
                Err(e) => return e.to_compile_error().into(),
            };
            let id_field = resolve_id(&input).unwrap_or_else(|| {
                Ident::new("id", proc_macro2::Span::call_site())
            });
            // Resolve id_type (defaults to String).
            let id_type = resolve_id_type(&input).unwrap_or_else(|| {
                syn::parse_quote!(String)
            });
            let field_ident = &field;
            let id_ident = &id_field;
            quote! {
                impl #impl_generics #crate_path::ConfigDiff for #db_name #ty_generics #where_clause {
                    type Id = #id_type;
                    fn diff(
                        old: &Self,
                        new: &Self,
                    ) -> (
                        std::collections::HashSet<#id_type>,
                        std::collections::HashSet<#id_type>,
                        std::collections::HashSet<#id_type>,
                    ) {
                        use std::collections::HashSet;
                        let old_ids: HashSet<#id_type> =
                            old.#field_ident.iter().map(|e| e.#id_ident.clone()).collect();
                        let new_ids: HashSet<#id_type> =
                            new.#field_ident.iter().map(|e| e.#id_ident.clone()).collect();
                        let added: HashSet<#id_type> =
                            new_ids.difference(&old_ids).cloned().collect();
                        let removed: HashSet<#id_type> =
                            old_ids.difference(&new_ids).cloned().collect();
                        let modified: HashSet<#id_type> = old_ids
                            .intersection(&new_ids)
                            .filter(|id| {
                                old.#field_ident.iter().find(|e| &e.#id_ident == *id)
                                    != new.#field_ident.iter().find(|e| &e.#id_ident == *id)
                            })
                            .cloned()
                            .collect();
                        (added, removed, modified)
                    }
                }
            }
        }
        syn::Data::Enum(_) => {
            // Enums use whole-value PartialEq diff - the `#[config_diff(...)]`
            // attribute (field/id/id_type) is only meaningful for struct Vec
            // fields. If a user wrote it on an enum, emit a clear compile
            // error instead of silently ignoring it.
            for attr in &input.attrs {
                if attr.path().is_ident("config_diff") {
                    return syn::Error::new(
                        attr.meta.span(),
                        "`#[config_diff(...)]` attributes are not supported on enums; \
                         enums use whole-value `PartialEq` diff (no field/id/id_type needed)",
                    )
                    .to_compile_error()
                    .into();
                }
            }
            // Enum: whole-value comparison via PartialEq. If old != new,
            // report the type name as the single modified id; otherwise
            // return an empty diff. This mirrors the manual "single config
            // object" pattern (e.g. `UiTheme`, `LevelAsset`).
            let type_name = db_name.to_string();
            quote! {
                impl #impl_generics #crate_path::ConfigDiff for #db_name #ty_generics #where_clause {
                    type Id = String;
                    fn diff(
                        old: &Self,
                        new: &Self,
                    ) -> (
                        std::collections::HashSet<String>,
                        std::collections::HashSet<String>,
                        std::collections::HashSet<String>,
                    ) {
                        use std::collections::HashSet;
                        if old != new {
                            let mut modified: HashSet<String> = HashSet::new();
                            modified.insert(#type_name.to_string());
                            (HashSet::new(), HashSet::new(), modified)
                        } else {
                            (HashSet::new(), HashSet::new(), HashSet::new())
                        }
                    }
                }
            }
        }
        _ => {
            return syn::Error::new(
                input.ident.span(),
                "ConfigDiff only supports structs and enums",
            )
            .to_compile_error()
            .into();
        }
    };

    expanded.into()
}

/// 从 `#[config_diff]` attribute 解析 field 名，或自动找第一个 Vec 字段。
fn resolve_field(input: &DeriveInput) -> syn::Result<Ident> {
    // 先找 helper attribute
    for attr in &input.attrs {
        if attr.path().is_ident("config_diff") {
            let attr_args: ConfigDiffAttr = attr.parse_args()?;
            if let Some(field) = attr_args.field {
                return Ok(field);
            }
        }
    }
    // 没有 attribute，自动找第一个 Vec<_> 字段
    let fields = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "ConfigDiff: field-based diff requires a struct with a Vec<_> field \
                 (enums use whole-value PartialEq diff; did you mean to derive on an enum?)",
            ))
        }
    };
    for field in fields.iter() {
        if is_vec_type(&field.ty) {
            if let Some(name) = &field.ident {
                return Ok(name.clone());
            }
        }
    }
    Err(syn::Error::new(
        input.ident.span(),
        "ConfigDiff: no `Vec<_>` field found; specify one via #[config_diff(field = \"name\")]",
    ))
}

/// 从 `#[config_diff]` attribute 解析 id 字段名（默认 "id"）。
fn resolve_id(input: &DeriveInput) -> Option<Ident> {
    for attr in &input.attrs {
        if attr.path().is_ident("config_diff") {
            if let Ok(attr_args) = attr.parse_args::<ConfigDiffAttr>() {
                return attr_args.id;
            }
        }
    }
    None
}

/// 从 `#[config_diff]` attribute 解析 id 字段类型（默认 `String`）。
///
/// 用法：`#[config_diff(field = "entries", id = "id", id_type = "u32")]`
/// 或 `#[config_diff(id_type = "uuid::Uuid")]` 等。
fn resolve_id_type(input: &DeriveInput) -> Option<Type> {
    for attr in &input.attrs {
        if attr.path().is_ident("config_diff") {
            if let Ok(attr_args) = attr.parse_args::<ConfigDiffAttr>() {
                return attr_args.id_type;
            }
        }
    }
    None
}

/// 判断类型是否是 `Vec<...>`。
fn is_vec_type(ty: &Type) -> bool {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            return seg.ident == "Vec";
        }
    }
    false
}
