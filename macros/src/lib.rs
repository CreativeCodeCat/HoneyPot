//! Macros for the bot's command framework (`src/core/framework`).
//!
//! `#[command(...)]` turns an `async fn(ctx, args...) -> Result<(), Error>` into a
//! `fn() -> Command` that registers it as a slash command (or context menu command) and parses
//! its options. `#[derive(Choice)]` lets a plain enum be a slash command option with choices.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprLit, FnArg, Ident, ItemFn, Lit, LitStr, Meta, Pat, Token, Type};

fn error(span: Span, message: &str) -> TokenStream {
    syn::Error::new(span, message).to_compile_error().into()
}

fn lit_str(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) => Ok(s.value()),
        _ => Err(syn::Error::new(expr.span(), "expected a string")),
    }
}

/// A number literal, possibly negative, as written.
fn lit_number(expr: &Expr) -> syn::Result<TokenStream2> {
    match expr {
        Expr::Lit(ExprLit { lit: Lit::Int(_) | Lit::Float(_), .. }) => Ok(quote!(#expr)),
        Expr::Unary(syn::ExprUnary { op: syn::UnOp::Neg(_), expr: inner, .. }) => {
            let inner = lit_number(inner)?;
            Ok(quote!(-#inner))
        }
        _ => Err(syn::Error::new(expr.span(), "expected a number")),
    }
}

/// The first paragraph of the doc comment, which is what Discord shows as the description.
fn description(attrs: &[Attribute]) -> Option<String> {
    let mut lines = vec![];
    for attr in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        let Meta::NameValue(nv) = &attr.meta else { continue };
        let Ok(line) = lit_str(&nv.value) else { continue };
        let line = line.trim().to_string();
        if line.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(line);
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// `"MANAGE_MESSAGES | BAN_MEMBERS"` as a `Permissions` expression.
fn permissions(text: &str) -> TokenStream2 {
    let flags = text.split('|').map(str::trim).filter(|f| !f.is_empty()).map(|f| format_ident!("{}", f));
    quote!(::serenity::all::Permissions::empty() #(| ::serenity::all::Permissions::#flags)*)
}

/// `Option<T>` -> `Some(T)`
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else { return None };
    match args.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn last_segment(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else { return None };
    path.path.segments.last().map(|s| s.ident.to_string())
}

#[derive(Default)]
struct CommandArgs {
    slash_command: bool,
    guild_only: bool,
    owners_only: bool,
    ephemeral: bool,
    subcommand_required: bool,
    category: Option<String>,
    rename: Option<String>,
    required_permissions: Option<String>,
    required_bot_permissions: Option<String>,
    default_member_permissions: Option<String>,
    context_menu_command: Option<String>,
    checks: Vec<Ident>,
    subcommands: Vec<Ident>,
}

fn parse_command_args(metas: Punctuated<Meta, Token![,]>) -> syn::Result<CommandArgs> {
    let mut args = CommandArgs::default();
    for meta in metas {
        let name = meta.path().get_ident().map(|i| i.to_string()).unwrap_or_default();
        match (&meta, name.as_str()) {
            (Meta::Path(_), "slash_command") => args.slash_command = true,
            (Meta::Path(_), "guild_only") => args.guild_only = true,
            (Meta::Path(_), "owners_only") => args.owners_only = true,
            (Meta::Path(_), "ephemeral") => args.ephemeral = true,
            (Meta::Path(_), "subcommand_required") => args.subcommand_required = true,
            (Meta::NameValue(nv), _) => {
                let value = lit_str(&nv.value)?;
                match name.as_str() {
                    "category" => args.category = Some(value),
                    "rename" => args.rename = Some(value),
                    "required_permissions" => args.required_permissions = Some(value),
                    "required_bot_permissions" => args.required_bot_permissions = Some(value),
                    "default_member_permissions" => args.default_member_permissions = Some(value),
                    "context_menu_command" => args.context_menu_command = Some(value),
                    "check" => args.checks.push(Ident::new(&value, nv.value.span())),
                    _ => return Err(syn::Error::new(meta.span(), format!("unknown command option `{name}`"))),
                }
            }
            (Meta::List(list), "subcommands") => {
                let names = list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
                args.subcommands = names.iter().map(|n| Ident::new(&n.value(), n.span())).collect();
            }
            _ => return Err(syn::Error::new(meta.span(), format!("unknown command option `{name}`"))),
        }
    }
    Ok(args)
}

struct Param {
    ident: Ident,
    name: String,
    ty: Type,
    /// The type without `Option<..>`
    inner: Type,
    required: bool,
    description: Option<String>,
    autocomplete: Option<Ident>,
    /// Builder calls that come after the type's own, e.g. `.max_length(100)`
    settings: Vec<TokenStream2>,
}

fn parse_param(arg: &mut syn::PatType, index: usize) -> syn::Result<Param> {
    let Pat::Ident(pat) = &*arg.pat else { return Err(syn::Error::new(arg.pat.span(), "expected a plain argument name")) };
    // `r#type` is called `type` in Discord
    let mut name = pat.ident.unraw().to_string().trim_start_matches('_').to_string();
    let mut description = None;
    let mut autocomplete = None;
    let mut settings = vec![];
    for attr in std::mem::take(&mut arg.attrs) {
        let key = attr.path().get_ident().map(|i| i.to_string()).unwrap_or_default();
        match (&attr.meta, key.as_str()) {
            (Meta::NameValue(nv), "description") => description = Some(lit_str(&nv.value)?),
            (Meta::NameValue(nv), "rename") => name = lit_str(&nv.value)?,
            (Meta::NameValue(nv), "autocomplete") => autocomplete = Some(Ident::new(&lit_str(&nv.value)?, nv.value.span())),
            (Meta::NameValue(nv), "min") => {
                let n = lit_number(&nv.value)?;
                settings.push(quote!(.min_number_value((#n) as f64)));
            }
            (Meta::NameValue(nv), "max") => {
                let n = lit_number(&nv.value)?;
                settings.push(quote!(.max_number_value((#n) as f64)));
            }
            (Meta::NameValue(nv), "min_length") => {
                let n = lit_number(&nv.value)?;
                settings.push(quote!(.min_length(#n)));
            }
            (Meta::NameValue(nv), "max_length") => {
                let n = lit_number(&nv.value)?;
                settings.push(quote!(.max_length(#n)));
            }
            (Meta::List(list), "channel_types") => {
                let kinds = list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
                let kinds = kinds.iter().map(|k| Ident::new(&k.value(), k.span()));
                settings.push(quote!(.channel_types(vec![#(::serenity::all::ChannelType::#kinds),*])));
            }
            _ => return Err(syn::Error::new(attr.span(), format!("unknown parameter option `{key}`"))),
        }
    }
    let ty = (*arg.ty).clone();
    let (inner, required) = match option_inner(&ty) {
        Some(inner) => (inner.clone(), false),
        None => (ty.clone(), true),
    };
    Ok(Param { ident: format_ident!("__arg{}", index), name, ty, inner, required, description, autocomplete, settings })
}

/// Turns an `async fn` into a `fn() -> Command`. Options: `slash_command`,
/// `context_menu_command = "Name"`, `guild_only`, `owners_only`, `ephemeral`,
/// `subcommands("a", "b")`, `subcommand_required`, `category = ".."`, `rename = ".."`,
/// `required_permissions = "A | B"`, `required_bot_permissions = ".."`,
/// `default_member_permissions = ".."` and `check = "function"`.
///
/// Parameters take `#[description = ".."]`, `#[rename = ".."]`, `#[min = n]`, `#[max = n]`,
/// `#[min_length = n]`, `#[max_length = n]`, `#[channel_types("Text", ..)]` and
/// `#[autocomplete = "function"]`.
#[proc_macro_attribute]
pub fn command(args: TokenStream, input: TokenStream) -> TokenStream {
    let metas = match syn::parse::Parser::parse(Punctuated::<Meta, Token![,]>::parse_terminated, args) {
        Ok(metas) => metas,
        Err(e) => return e.to_compile_error().into(),
    };
    let args = match parse_command_args(metas) {
        Ok(args) => args,
        Err(e) => return e.to_compile_error().into(),
    };
    let mut function = match syn::parse::<ItemFn>(input) {
        Ok(function) => function,
        Err(e) => return e.to_compile_error().into(),
    };
    if function.sig.asyncness.is_none() {
        return error(function.sig.span(), "commands must be async");
    }
    if !args.slash_command && args.context_menu_command.is_none() {
        return error(function.sig.span(), "a command needs `slash_command` or `context_menu_command`");
    }

    let mut params = vec![];
    for (index, input) in function.sig.inputs.iter_mut().enumerate().skip(1) {
        let FnArg::Typed(arg) = input else { return error(input.span(), "unexpected `self`") };
        match parse_param(arg, index) {
            Ok(param) => params.push(param),
            Err(e) => return e.to_compile_error().into(),
        }
    }

    let fn_name = function.sig.ident.clone();
    let vis = function.vis.clone();
    let name = args.rename.clone().unwrap_or_else(|| fn_name.to_string());
    let description = match description(&function.attrs) {
        Some(d) => quote!(Some(#d.to_string())),
        None => quote!(None),
    };
    let category = match &args.category {
        Some(c) => quote!(Some(#c.to_string())),
        None => quote!(None),
    };
    let none = || quote!(::serenity::all::Permissions::empty());
    let required_permissions = args.required_permissions.as_deref().map_or_else(none, permissions);
    let required_bot_permissions = args.required_bot_permissions.as_deref().map_or_else(none, permissions);
    let default_member_permissions = args.default_member_permissions.as_deref().map_or_else(none, permissions);
    let (guild_only, owners_only, ephemeral, subcommand_required) =
        (args.guild_only, args.owners_only, args.ephemeral, args.subcommand_required);
    let checks = &args.checks;
    let subcommands = &args.subcommands;

    let parameters = params.iter().map(|p| {
        let (name, inner, required, settings) = (&p.name, &p.inner, p.required, &p.settings);
        let description = p.description.clone().unwrap_or_else(|| name.clone());
        let autocomplete = match &p.autocomplete {
            Some(f) => quote!(Some(|ctx, partial| Box::pin(async move {
                crate::core::framework::IntoChoices::into_choices(#f(ctx, partial).await)
            }))),
            None => quote!(None),
        };
        let autocomplete_setting = p.autocomplete.as_ref().map(|_| quote!(.set_autocomplete(true)));
        quote!(crate::core::framework::Parameter {
            name: #name.to_string(),
            description: #description.to_string(),
            required: #required,
            create: |option| <#inner as crate::core::framework::SlashArg>::create(option) #(#settings)* #autocomplete_setting,
            autocomplete: #autocomplete,
        })
    });

    let idents: Vec<_> = params.iter().map(|p| &p.ident).collect();
    let slash_action = if args.slash_command {
        let extracts = params.iter().map(|p| {
            let (ident, name, ty, inner) = (&p.ident, &p.name, &p.ty, &p.inner);
            if p.required {
                quote!(let #ident: #ty = crate::core::framework::required_arg::<#inner>(ctx, #name).await?;)
            } else {
                quote!(let #ident: #ty = crate::core::framework::optional_arg::<#inner>(ctx, #name).await?;)
            }
        });
        quote!(Some(|ctx| Box::pin(async move {
            #(#extracts)*
            inner(ctx #(, #idents)*).await
        })))
    } else {
        quote!(None)
    };
    let (context_menu_action, context_menu_name) = match &args.context_menu_command {
        Some(menu_name) => {
            let [param] = params.as_slice() else {
                return error(function.sig.span(), "a context menu command takes exactly one argument, a User or a Message");
            };
            let action = match last_segment(&param.ty).as_deref() {
                Some("User") => quote!(User),
                Some("Message") => quote!(Message),
                _ => return error(param.ty.span(), "a context menu command's argument must be a User or a Message"),
            };
            (
                quote!(Some(crate::core::framework::ContextMenuAction::#action(|ctx, target| Box::pin(inner(ctx, target))))),
                quote!(Some(#menu_name.to_string())),
            )
        }
        None => (quote!(None), quote!(None)),
    };

    // The original function, renamed, without the attributes on its parameters
    let doc_attrs: Vec<_> = function.attrs.iter().filter(|a| a.path().is_ident("doc")).cloned().collect();
    function.attrs.retain(|a| !a.path().is_ident("doc"));
    function.sig.ident = Ident::new("inner", fn_name.span());
    function.vis = syn::Visibility::Inherited;

    quote! {
        #(#doc_attrs)*
        #vis fn #fn_name() -> crate::core::framework::Command {
            #function

            crate::core::framework::Command {
                name: #name.to_string(),
                qualified_name: #name.to_string(),
                description: #description,
                category: #category,
                guild_only: #guild_only,
                owners_only: #owners_only,
                ephemeral: #ephemeral,
                subcommand_required: #subcommand_required,
                required_permissions: #required_permissions,
                required_bot_permissions: #required_bot_permissions,
                default_member_permissions: #default_member_permissions,
                checks: vec![#(|ctx| Box::pin(#checks(ctx))),*],
                subcommands: vec![#(#subcommands()),*],
                parameters: vec![#(#parameters),*],
                slash_action: #slash_action,
                context_menu_action: #context_menu_action,
                context_menu_name: #context_menu_name,
            }
        }
    }
    .into()
}

/// Makes a fieldless enum a slash command option whose choices are its variants. A variant
/// shows as its name, or as `#[name = ".."]`.
#[proc_macro_derive(Choice, attributes(name))]
pub fn derive_choice(input: TokenStream) -> TokenStream {
    let input = match syn::parse::<syn::DeriveInput>(input) {
        Ok(input) => input,
        Err(e) => return e.to_compile_error().into(),
    };
    let syn::Data::Enum(data) = &input.data else { return error(input.span(), "Choice can only be derived for enums") };
    let mut variants = vec![];
    let mut names = vec![];
    for variant in &data.variants {
        if !matches!(variant.fields, syn::Fields::Unit) {
            return error(variant.span(), "Choice variants can't have fields");
        }
        let mut name = variant.ident.to_string();
        for attr in variant.attrs.iter().filter(|a| a.path().is_ident("name")) {
            match &attr.meta {
                Meta::NameValue(nv) => match lit_str(&nv.value) {
                    Ok(n) => name = n,
                    Err(e) => return e.to_compile_error().into(),
                },
                _ => return error(attr.span(), "expected #[name = \"..\"]"),
            }
        }
        variants.push(variant.ident.clone());
        names.push(name);
    }
    let ident = &input.ident;
    let indices: Vec<i64> = (0..variants.len() as i64).collect();
    quote! {
        impl crate::core::framework::Choice for #ident {
            fn name(&self) -> &'static str {
                match self {
                    #(Self::#variants => #names,)*
                }
            }
        }

        impl crate::core::framework::SlashArg for #ident {
            fn create(option: ::serenity::all::CreateCommandOption) -> ::serenity::all::CreateCommandOption {
                option.kind(::serenity::all::CommandOptionType::Integer)
                    #(.add_int_choice(#names, #indices as i32))*
            }

            fn extract<'a>(
                _ctx: crate::core::framework::Context<'a>,
                value: &'a ::serenity::all::ResolvedValue<'a>,
            ) -> crate::core::framework::BoxFuture<'a, Result<Self, String>> {
                Box::pin(async move {
                    match value {
                        #(::serenity::all::ResolvedValue::Integer(#indices) => Ok(Self::#variants),)*
                        _ => Err("isn't one of the choices".to_string()),
                    }
                })
            }
        }
    }
    .into()
}
