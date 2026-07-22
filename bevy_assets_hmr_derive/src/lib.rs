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
use quote::{ToTokens, quote};
use syn::spanned::Spanned;
use syn::{DeriveInput, Ident, LitStr, Type, parse_macro_input};

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
                    ));
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
            let id_field = resolve_id(&input)
                .unwrap_or_else(|| Ident::new("id", proc_macro2::Span::call_site()));
            // Resolve id_type (defaults to String).
            let id_type = resolve_id_type(&input).unwrap_or_else(|| syn::parse_quote!(String));
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
                        #crate_path::diff_entries_by_id(
                            &old.#field_ident,
                            &new.#field_ident,
                            |entry| entry.#id_ident.clone(),
                        )
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
            ));
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

// ===========================================================================
// HmrAutoWatch derive：兼容 bevy_asset_loader
// ===========================================================================

/// Helper attribute for `#[derive(HmrAutoWatch)]`: `#[hmr(skip)]`.
struct HmrFieldAttr {
    skip: bool,
}

impl syn::parse::Parse for HmrFieldAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Err(input.error("expected `skip`; use `#[hmr_watch]` to watch a direct asset"));
        }
        let mut skip = false;
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "skip" => skip = true,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown #[hmr] option `{}`; expected `skip`", other),
                    ));
                }
            }
            let _ = input.parse::<syn::Token![,]>();
        }
        Ok(Self { skip })
    }
}

/// 单个 HMR 字段的元信息。
struct HmrField {
    /// 字段 ident（如 `cfg`）
    ident: Ident,
    /// Handle 内层类型 token（如 `ConfigAsset<MyConfig>`、`LevelAsset`）
    asset_type: Type,
    /// `#[asset(path = "...")]` 中的 path 字面量
    path: LitStr,
}

/// 从字段属性中提取 `#[asset(path = "...")]` 的 path 值。
///
/// 使用 `syn::Meta` 解析整个属性，因此也支持
/// `#[asset(path = "...", optional)]` 这类包含其他选项的标准写法。
fn extract_asset_path(field: &syn::Field) -> syn::Result<Option<LitStr>> {
    for attr in &field.attrs {
        if attr.path().is_ident("asset") {
            let entries = attr.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )?;
            for entry in entries {
                if let syn::Meta::NameValue(name_value) = entry {
                    if !name_value.path.is_ident("path") {
                        continue;
                    }
                    let syn::Expr::Lit(expr_lit) = name_value.value else {
                        return Err(syn::Error::new(
                            name_value.path.span(),
                            "`asset(path = ...)` must use a string literal for HMR",
                        ));
                    };
                    let syn::Lit::Str(path) = expr_lit.lit else {
                        return Err(syn::Error::new(
                            expr_lit.lit.span(),
                            "`asset(path = ...)` must use a string literal for HMR",
                        ));
                    };
                    return Ok(Some(path));
                }
            }
        }
    }
    Ok(None)
}

/// 从字段类型 `Handle<A>` 中提取内层 `A`。
/// 支持 `Handle<ConfigAsset<T>>`、`Handle<MyAsset>` 等。
fn extract_handle_inner_type(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let last_seg = type_path.path.segments.last()?;
    if last_seg.ident != "Handle" {
        return None;
    }
    // 取 Handle<...> 的泛型参数
    let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments else {
        return None;
    };
    for arg in &args.args {
        if let syn::GenericArgument::Type(inner_ty) = arg {
            return Some(inner_ty.clone());
        }
    }
    None
}

/// 判断类型是否是 `ConfigAsset<T>`，若是则返回 `T` 的 token。
/// 支持 `ConfigAsset<MyConfig>` 等。
fn extract_config_asset_inner(ty: &Type) -> Option<Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    let last_seg = type_path.path.segments.last()?;
    if last_seg.ident != "ConfigAsset" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments else {
        return None;
    };
    for arg in &args.args {
        if let syn::GenericArgument::Type(inner_ty) = arg {
            return Some(inner_ty.clone());
        }
    }
    None
}

#[proc_macro_derive(HmrAutoWatch, attributes(hmr, hmr_watch))]
pub fn derive_hmr_auto_watch(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let crate_path = quote! { ::bevy_assets_hmr };
    let db_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // 只支持 struct
    let syn::Data::Struct(data_struct) = &input.data else {
        return syn::Error::new(input.ident.span(), "HmrAutoWatch only supports structs")
            .to_compile_error()
            .into();
    };

    // 收集所有需要 HMR 接管的字段。
    // - Handle<ConfigAsset<T>> + 静态 path：默认接入（包装模式）
    // - 其他 Handle<A>：仅在显式 #[hmr_watch] 时接入（直接模式）
    // - #[hmr(skip)]：显式跳过，包括 ConfigAsset<T>
    let mut hmr_fields: Vec<HmrField> = Vec::new();
    let mut errors: Option<syn::Error> = None;
    for field in data_struct.fields.iter() {
        let Some(field_ident) = &field.ident else {
            continue;
        };

        let explicitly_watched = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("hmr_watch"));

        // 检查 #[hmr(skip)]，并保留解析错误而不是静默忽略。
        let mut skip = false;
        for attr in &field.attrs {
            if attr.path().is_ident("hmr") {
                match attr.parse_args::<HmrFieldAttr>() {
                    Ok(hmr_attr) => skip = hmr_attr.skip,
                    Err(error) => {
                        if let Some(errors) = &mut errors {
                            errors.combine(error);
                        } else {
                            errors = Some(error);
                        }
                    }
                }
            }
        }
        if skip && explicitly_watched {
            let error = syn::Error::new(
                field.span(),
                "`#[hmr_watch]` and `#[hmr(skip)]` cannot be used on the same field",
            );
            if let Some(errors) = &mut errors {
                errors.combine(error);
            } else {
                errors = Some(error);
            }
            continue;
        }
        if skip {
            continue;
        }

        // 必须是 Handle<A> 类型。未标记的普通字段直接跳过；显式标记则报错。
        let Some(inner_type) = extract_handle_inner_type(&field.ty) else {
            if explicitly_watched {
                let error = syn::Error::new(
                    field.ty.span(),
                    "`#[hmr_watch]` can only be used on a `Handle<A>` field",
                );
                if let Some(errors) = &mut errors {
                    errors.combine(error);
                } else {
                    errors = Some(error);
                }
            }
            continue;
        };

        let is_config_asset = extract_config_asset_inner(&inner_type).is_some();

        // 普通 Handle<A> 未显式 opt in 时立即跳过，不解析或限制它的 #[asset(...)]
        // 语法，确保 HmrAutoWatch 不干扰 bevy_asset_loader 的其他资产字段。
        if !explicitly_watched && !is_config_asset {
            continue;
        }

        let path = match extract_asset_path(field) {
            Ok(path) => path,
            Err(error) => {
                if let Some(errors) = &mut errors {
                    errors.combine(error);
                } else {
                    errors = Some(error);
                }
                continue;
            }
        };

        // 包装模式只有静态 path 时才默认接入，以免干扰 dynamic/key/collection 字段。
        // 直接模式必须显式 #[hmr_watch]，这样普通 Handle<Image> 会自然跳过。
        if !explicitly_watched && path.is_none() {
            continue;
        }

        let Some(path) = path else {
            let error = syn::Error::new(
                field.span(),
                "`#[hmr_watch]` requires `#[asset(path = \"...\")]`; dynamic asset keys are not supported",
            );
            if let Some(errors) = &mut errors {
                errors.combine(error);
            } else {
                errors = Some(error);
            }
            continue;
        };

        hmr_fields.push(HmrField {
            ident: field_ident.clone(),
            asset_type: inner_type,
            path,
        });
    }

    if let Some(errors) = errors {
        return errors.to_compile_error().into();
    }

    if hmr_fields.is_empty() {
        return syn::Error::new(
            input.ident.span(),
            "HmrAutoWatch found no watchable fields; use `Handle<ConfigAsset<T>>` with a static `#[asset(path = \"...\")]`, mark a direct `Handle<A: HmrSource>` with `#[hmr_watch]`, or remove the derive",
        )
        .to_compile_error()
        .into();
    }

    // 生成字段安装代码片段（在 OnEnter system 中调用 adopt_handle）
    // 设计：plugin build 阶段（有 &mut App）对每个字段调用 register_*_impl(autoload=false)
    //   注册 HMR 框架系统；OnEnter system 运行时拿 &mut World，调用 adopt_handle
    //   持有 handle + 预热快照。
    //
    // 包装模式（Handle<ConfigAsset<T>>）用 register_config_impl::<T>；
    // 直接模式（Handle<A: HmrSource>）用 register_asset_impl::<A>。

    // 为每个字段生成 build 阶段的注册代码
    let mut registered_types = std::collections::HashSet::new();
    let register_calls: Vec<TokenStream2> = hmr_fields
        .iter()
        .filter_map(|f| {
            let type_key = f.asset_type.to_token_stream().to_string();
            if !registered_types.insert(type_key) {
                return None;
            }
            let path_lit = &f.path;
            if let Some(config_t) = extract_config_asset_inner(&f.asset_type) {
                // 包装模式：ConfigAsset<T> -> register_config_impl::<T>
                Some(quote! {
                    #crate_path::ext::register_config_impl::<#config_t>(
                        app,
                        #path_lit,
                        false,
                    );
                })
            } else {
                // 直接模式：A -> register_asset_impl::<A>
                let asset_ty = &f.asset_type;
                Some(quote! {
                    #crate_path::ext::register_asset_impl::<#asset_ty>(
                        app,
                        #path_lit,
                        false,
                    );
                })
            }
        })
        .collect();

    // 为每个字段生成 OnEnter system 中的 adopt_handle 调用
    let field_installs_in_system: Vec<TokenStream2> = hmr_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let asset_ty = &f.asset_type;
            let path_lit = &f.path;
            quote! {
                {
                    let handle = assets.#ident.clone();
                    #crate_path::ext::adopt_handle::<#asset_ty>(
                        world,
                        handle,
                        #path_lit,
                    );
                }
            }
        })
        .collect();

    let expanded: TokenStream2 = quote! {
        impl #impl_generics #crate_path::HmrAutoWatch for #db_name #ty_generics #where_clause {}

        impl #impl_generics #db_name #ty_generics #where_clause {
            /// 返回一个 Plugin，在进入指定状态时把所有 `Handle<A: HmrSource>` 字段
            /// 接入 HMR 框架。
            ///
            /// `state` 通常是 `LoadingState` 的 `continue_to_state` 目标状态
            /// （如 `GameState::Ready`），表示资产已加载完毕。
            pub fn hmr_plugin<S: ::bevy::state::state::States + Clone>(state: S) -> #crate_path::HmrAutoWatchPlugin {
                #crate_path::HmrAutoWatchPlugin::new(::std::boxed::Box::new(move |app: &mut ::bevy::app::App| {
                    // build 阶段：预先注册 HMR 框架系统（不加载文件）
                    #(
                        #register_calls
                    )*

                    // 注册 OnEnter(state) system：加载完成后接管 handle
                    let state_clone = state.clone();
                    app.add_systems(
                        ::bevy::state::prelude::OnEnter::<S>(state_clone),
                        |world: &mut ::bevy::ecs::world::World| {
                            let assets = world.resource::<#db_name #ty_generics>().clone();
                            #(
                                #field_installs_in_system
                            )*
                        },
                    );
                }))
            }
        }
    };

    expanded.into()
}
