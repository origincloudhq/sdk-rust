//! The clients: one per service, with one async method per RPC, sent as its
//! `google.api.http` binding with proto3's JSON mapping through the core
//! crate.
//!
//! A method answering with a long-running operation returns an
//! `Operation` of the result its `operation_info` names. A list method
//! (AIP-158) also gets an `all_...` method streaming every page. A method
//! with a `request_id` gets one filled in when it is empty, so that it is
//! safe to retry.

use std::fmt::Write as _;

use anyhow::{Context as _, Result, bail, ensure};
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor,
    ServiceDescriptor, Value,
};

use crate::docs::{self, Comments};
use crate::names;

/// The services that answer only at their regionless host.
const GLOBAL_SERVICES: &[&str] = &["iam", "admin", "location"];

const OPERATION: &str = "google.longrunning.Operation";
const EMPTY: &str = "google.protobuf.Empty";

/// Where the generated code of one package finds the rest.
pub struct Generation<'a> {
    pub pool: &'a DescriptorPool,
    pub comments: &'a Comments,
    /// The proto package, such as `origincloud.pvs.v1beta1`.
    pub package: &'a str,
    /// The path of the core crate: `crate` in it, `::origincloud_core`
    /// elsewhere.
    pub core: &'a str,
}

impl Generation<'_> {
    /// The Rust path of a message or enum, from the package's module.
    pub fn rust_type(&self, full_name: &str) -> Result<String> {
        let core = self.core;
        if full_name == OPERATION {
            return Ok(format!("{core}::longrunning::Operation"));
        }
        if full_name
            .strip_prefix(self.package)
            .is_some_and(|rest| rest.starts_with('.'))
        {
            return Ok(names::relative_type_path(self.package, full_name));
        }
        if let Some(name) = full_name.strip_prefix("google.protobuf.") {
            return Ok(format!("{core}::protobuf::{}", names::upper_camel(name)));
        }
        if let Some(rest) = full_name.strip_prefix("origincloud.longrunning.") {
            let (version, _) = rest.split_once('.').context("a type outside a version")?;
            let package = format!("origincloud.longrunning.{version}");
            return Ok(format!(
                "{core}::longrunning::{version}::{}",
                names::relative_type_path(&package, full_name)
            ));
        }
        bail!(
            "{full_name} is in a package {} can't refer to",
            self.package
        )
    }
}

/// The client code of every service of the package's files.
pub fn generate(cx: &Generation<'_>) -> Result<String> {
    let mut out = String::new();
    let mut files: Vec<_> = cx
        .pool
        .files()
        .filter(|f| f.package_name() == cx.package)
        .collect();
    files.sort_by(|a, b| a.name().cmp(b.name()));
    for file in files {
        for service in file.services() {
            generate_service(cx, &service, &mut out)
                .with_context(|| service.full_name().to_owned())?;
        }
    }
    Ok(out)
}

fn generate_service(cx: &Generation<'_>, s: &ServiceDescriptor, out: &mut String) -> Result<()> {
    let core = cx.core;
    let client = format!(
        "{}Client",
        s.name().strip_suffix("Service").unwrap_or(s.name())
    );
    let file = s.parent_file();

    out.push_str(&docs::line(&format!("Calls `{}`.", s.name())));
    let comment = cx.comments.leading(file.name(), s.path());
    if !comment.is_empty() {
        out.push_str(&docs::line(""));
        out.push_str(&docs::attributes(comment));
    }
    writeln!(out, "#[derive(Clone, Debug)]")?;
    writeln!(out, "pub struct {client} {{ service: {core}::Service }}")?;
    writeln!(out, "impl {client} {{")?;

    let host = default_host(cx.pool, s);
    if let Some(host) = host {
        let shortcode = host.split('.').next().unwrap_or_default();
        let regional = !GLOBAL_SERVICES.contains(&shortcode);
        out.push_str(&docs::line(&format!(
            "Returns a client of `{}` that sends its calls through `client`.",
            s.name()
        )));
        writeln!(
            out,
            "pub fn new(client: &{core}::Client) -> Self {{ Self {{ service: client.service({shortcode:?}, {regional}) }} }}"
        )?;
        out.push_str(&docs::line(
            "The endpoint the client sends its calls to, which also serves the operations its methods answer with.",
        ));
        writeln!(
            out,
            "pub fn service(&self) -> &{core}::Service {{ &self.service }}"
        )?;
    } else {
        out.push_str(&docs::line(&format!(
            "Returns a client of `{}` at `service`, the endpoint of another service's client: every service serves it at its own host.",
            s.name()
        )));
        writeln!(
            out,
            "pub fn new(service: &{core}::Service) -> Self {{ Self {{ service: service.clone() }} }}"
        )?;
    }

    for m in s.methods() {
        generate_method(cx, &m, out).with_context(|| m.name().to_owned())?;
        if let Some(items) = list_items(&m) {
            generate_all(cx, &m, &items, out)?;
        }
    }
    writeln!(out, "}}")?;
    Ok(())
}

fn default_host(pool: &DescriptorPool, s: &ServiceDescriptor) -> Option<String> {
    let ext = pool.get_extension_by_name("google.api.default_host")?;
    let options = s.options();
    if !options.has_extension(&ext) {
        return None;
    }
    options.get_extension(&ext).as_str().map(str::to_owned)
}

/// The message an operation's result is, for a method answering with a
/// long-running operation.
pub fn operation_result(pool: &DescriptorPool, m: &MethodDescriptor) -> Option<MessageDescriptor> {
    if m.output().full_name() != OPERATION {
        return None;
    }
    let info = message_extension(pool, &m.options(), "google.longrunning.operation_info")?;
    let name = info
        .get_field_by_name("response_type")?
        .as_str()?
        .to_owned();
    let name = if name.contains('.') {
        name
    } else {
        format!("{}.{name}", m.parent_file().package_name())
    };
    pool.get_message_by_name(&name)
}

fn message_extension(
    pool: &DescriptorPool,
    options: &DynamicMessage,
    name: &str,
) -> Option<DynamicMessage> {
    let ext = pool.get_extension_by_name(name)?;
    if !options.has_extension(&ext) {
        return None;
    }
    match options.get_extension(&ext).into_owned() {
        Value::Message(m) => Some(m),
        _ => None,
    }
}

/// A piece of a path template: a literal, or a variable binding a field.
enum Segment {
    Literal(String),
    Variable {
        fields: Vec<FieldDescriptor>,
        pattern: String,
    },
}

fn generate_method(cx: &Generation<'_>, m: &MethodDescriptor, out: &mut String) -> Result<()> {
    ensure!(
        !m.is_client_streaming() && !m.is_server_streaming(),
        "streaming methods aren't supported"
    );
    let core = cx.core;
    let rule = message_extension(cx.pool, &m.options(), "google.api.http")
        .context("no google.api.http binding")?;
    if let Some(Value::List(more)) = rule.get_field_by_name("additional_bindings").as_deref() {
        ensure!(more.is_empty(), "one binding with a path is supported");
    }
    let (verb, template) = binding(&rule)?;
    let segments = parse_template(&m.input(), &template)?;

    let input = cx.rust_type(m.input().full_name())?;
    let result = operation_result(cx.pool, m);
    if m.output().full_name() == OPERATION && result.is_none() && !is_operation_service(m) {
        bail!("it answers with an operation but names no operation_info response_type");
    }
    let output = match &result {
        Some(r) => format!("{core}::Operation<{}>", cx.rust_type(r.full_name())?),
        None => cx.rust_type(m.output().full_name())?,
    };

    let name = names::snake(m.name());
    let comment = cx.comments.leading(m.parent_file().name(), m.path());
    if comment.is_empty() {
        out.push_str(&docs::line(&format!("Calls `{}`.", m.name())));
    } else {
        out.push_str(&docs::attributes(comment));
    }
    if let Some(r) = &result {
        let link = core.trim_start_matches("::");
        out.push_str(&docs::line(""));
        if r.full_name() == EMPTY {
            out.push_str(&docs::line(&format!(
                "It answers with an operation to wait on, which has no result: [`Operation::wait`]({link}::Operation::wait) returns once the change is done."
            )));
        } else {
            out.push_str(&docs::line(&format!(
                "It answers with an operation to wait on: [`Operation::wait`]({link}::Operation::wait) returns the [`{}`] once the change is done.",
                cx.rust_type(r.full_name())?
            )));
        }
    }

    let request_id = m
        .input()
        .get_field_by_name("request_id")
        .filter(|f| f.kind() == Kind::String && !f.is_list());
    let idempotent = verb == "GET" || request_id.is_some() || declared_idempotent(m);
    let binding = if request_id.is_some() {
        "mut request"
    } else {
        "request"
    };
    writeln!(
        out,
        "pub async fn {name}(&self, {binding}: {input}) -> ::core::result::Result<{output}, {core}::Error> {{"
    )?;
    if let Some(field) = &request_id {
        let f = names::snake(field.name());
        writeln!(
            out,
            "if request.{f}.is_empty() {{ request.{f} = {core}::new_request_id(); }}"
        )?;
    }

    let mut path = String::new();
    let mut bound = Vec::new();
    let mut variable = 0;
    for segment in &segments {
        match segment {
            Segment::Literal(literal) => {
                path.push_str(&literal.replace('{', "{{").replace('}', "}}"))
            }
            Segment::Variable { fields, pattern } => {
                let json_path = fields
                    .iter()
                    .map(|f| f.json_name())
                    .collect::<Vec<_>>()
                    .join(".");
                writeln!(
                    out,
                    "let p{variable} = {core}::path_value({json_path:?}, {}, {pattern:?})?;",
                    field_access(fields)
                )?;
                write!(path, "{{p{variable}}}")?;
                bound.push(json_path);
                variable += 1;
            }
        }
    }

    let body = rule
        .get_field_by_name("body")
        .and_then(|b| b.as_str().map(str::to_owned))
        .unwrap_or_default();
    let path = if variable == 0 {
        format!(
            "::std::string::String::from({:?})",
            path.replace("{{", "{").replace("}}", "}")
        )
    } else {
        format!("format!({path:?})")
    };
    let mut call = format!("{core}::Request::new({core}::Method::{verb}, {path})");
    match body.as_str() {
        "*" => {
            writeln!(out, "let body = {core}::json_body(&request)?;")?;
            call.push_str(".with_body(body)");
        }
        _ => {
            let mut skip = bound;
            if !body.is_empty() {
                let field = m.input().get_field_by_name(&body).with_context(|| {
                    format!("the body field {body} isn't a field of the request")
                })?;
                ensure!(
                    matches!(field.kind(), Kind::Message(_)) && !field.is_list() && !field.is_map(),
                    "the body field {body} isn't a message"
                );
                writeln!(
                    out,
                    "let body = {core}::json_body(request.{}.as_ref().unwrap_or(&::core::default::Default::default()))?;",
                    names::snake(field.name())
                )?;
                call.push_str(".with_body(body)");
                skip.push(field.json_name().to_owned());
            }
            writeln!(out, "let query = {core}::query(&request, &{skip:?})?;")?;
            call.push_str(".with_query(query)");
        }
    }
    if idempotent {
        call.push_str(".idempotent()");
    }
    writeln!(out, "let call = {call};")?;
    match &result {
        Some(r) => {
            writeln!(out, "let operation = self.service.call(call).await?;")?;
            writeln!(
                out,
                "Ok({core}::Operation::new(self.service.clone(), operation, {:?}))",
                r.full_name()
            )?;
        }
        None => writeln!(out, "self.service.call(call).await")?,
    }
    writeln!(out, "}}")?;
    Ok(())
}

/// Whether m is one of the methods reading operations themselves, which
/// answer with an operation without starting one.
/// Whether the API declares the method safe to repeat with
/// `option idempotency_level`, as a read that isn't a GET (WaitOperation)
/// does.
fn declared_idempotent(m: &MethodDescriptor) -> bool {
    use prost_types::method_options::IdempotencyLevel;
    m.method_descriptor_proto()
        .options
        .as_ref()
        .is_some_and(|o| o.idempotency_level() != IdempotencyLevel::IdempotencyUnknown)
}

fn is_operation_service(m: &MethodDescriptor) -> bool {
    m.parent_service().name() == "OperationService"
}

/// A list method (AIP-158): its request takes a page token, and its
/// response carries the next one and one repeated field of messages.
fn list_items(m: &MethodDescriptor) -> Option<FieldDescriptor> {
    m.input().get_field_by_name("page_token")?;
    m.output().get_field_by_name("next_page_token")?;
    let output = m.output();
    let mut items = output
        .fields()
        .filter(|f| f.is_list() && matches!(f.kind(), Kind::Message(_)));
    let first = items.next()?;
    items.next().is_none().then_some(first)
}

fn generate_all(
    cx: &Generation<'_>,
    m: &MethodDescriptor,
    items: &FieldDescriptor,
    out: &mut String,
) -> Result<()> {
    let core = cx.core;
    let Kind::Message(item) = items.kind() else {
        unreachable!("list_items returns message fields");
    };
    let item = cx.rust_type(item.full_name())?;
    let input = cx.rust_type(m.input().full_name())?;
    let list = names::snake(m.name());
    let field = names::snake(items.name());
    let name = format!("all_{}", items.name().to_lowercase());
    out.push_str(&docs::line(&format!(
        "Returns everything [`{list}`](Self::{list}) lists from `request`'s page on, fetching the next page as the stream reaches it. A page that fails ends the stream with its error."
    )));
    writeln!(
        out,
        "pub fn {name}(&self, request: {input}) -> {core}::Pages<{item}> {{"
    )?;
    writeln!(out, "let client = self.clone();")?;
    writeln!(
        out,
        "{core}::Pages::new(request, move |request: {input}| {{ let client = client.clone(); async move {{ let page = client.{list}(request).await?; Ok((page.{field}, page.next_page_token)) }} }}, |request: &mut {input}, token: ::std::string::String| request.page_token = token)"
    )?;
    writeln!(out, "}}")?;
    Ok(())
}

fn binding(rule: &DynamicMessage) -> Result<(&'static str, String)> {
    for (field, verb) in [
        ("get", "GET"),
        ("post", "POST"),
        ("put", "PUT"),
        ("patch", "PATCH"),
        ("delete", "DELETE"),
    ] {
        if rule.has_field_by_name(field) {
            let template = rule
                .get_field_by_name(field)
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            ensure!(!template.is_empty(), "the binding has no path");
            return Ok((verb, template));
        }
    }
    bail!("only get, post, put, patch and delete bindings are supported")
}

/// Splits a path template, such as
/// `/v1beta1/{name=projects/*/zones/*/volumes/*}:wait`, into literals and
/// the fields its variables bind.
fn parse_template(input: &MessageDescriptor, template: &str) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            segments.push(Segment::Literal(rest.to_owned()));
            break;
        };
        if open > 0 {
            segments.push(Segment::Literal(rest[..open].to_owned()));
        }
        let close = rest[open..]
            .find('}')
            .map(|i| i + open)
            .with_context(|| format!("unbalanced braces in {template}"))?;
        let variable = &rest[open + 1..close];
        let (name, pattern) = variable.split_once('=').unwrap_or((variable, "*"));
        ensure!(!pattern.contains("**"), "{template}: ** isn't supported");
        segments.push(Segment::Variable {
            fields: resolve_field(input, name)?,
            pattern: pattern.to_owned(),
        });
        rest = &rest[close + 1..];
    }
    Ok(segments)
}

/// The fields a dotted path names, ending in a string field.
fn resolve_field(message: &MessageDescriptor, path: &str) -> Result<Vec<FieldDescriptor>> {
    let mut fields: Vec<FieldDescriptor> = Vec::new();
    let mut current = message.clone();
    for (i, name) in path.split('.').enumerate() {
        if i > 0 {
            let Kind::Message(next) = fields[i - 1].kind() else {
                bail!("the path variable {path} goes through a field that isn't a message");
            };
            current = next;
        }
        let field = current
            .get_field_by_name(name)
            .with_context(|| format!("the path variable {path} isn't a field"))?;
        fields.push(field);
    }
    let last = fields.last().context("an empty path variable")?;
    ensure!(
        last.kind() == Kind::String && !last.is_list(),
        "the path variable {path} isn't a string field"
    );
    Ok(fields)
}

/// The expression reading a path of fields from `request`, as a `&str`.
fn field_access(fields: &[FieldDescriptor]) -> String {
    let names: Vec<String> = fields.iter().map(|f| names::snake(f.name())).collect();
    match names.as_slice() {
        [only] => format!("&request.{only}"),
        [first, middle @ .., last] => {
            let mut expr = format!("request.{first}.as_ref()");
            for m in middle {
                write!(expr, ".and_then(|m| m.{m}.as_ref())").expect("writing to a string");
            }
            write!(expr, ".map_or(\"\", |m| m.{last}.as_str())").expect("writing to a string");
            expr
        }
        [] => unreachable!("resolve_field returns at least one field"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use protox::file::{ChainFileResolver, GoogleFileResolver};

    use super::*;
    use crate::MemoryResolver;

    const SCRATCH: &str = r#"
        syntax = "proto3";
        package origincloud.scratch.v1;
        message Request { string name = 1; }
        message Response {}
        service Scratch {
          rpc Wait(Request) returns (Response) { option idempotency_level = NO_SIDE_EFFECTS; }
          rpc Set(Request) returns (Response) { option idempotency_level = IDEMPOTENT; }
          rpc Change(Request) returns (Response);
        }
    "#;

    #[test]
    fn a_declared_idempotency_level_makes_a_method_repeatable() {
        let mut resolver = ChainFileResolver::new();
        resolver.add(MemoryResolver(BTreeMap::from([(
            "scratch.proto".to_owned(),
            SCRATCH.to_owned(),
        )])));
        resolver.add(GoogleFileResolver::new());
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler
            .open_files(["scratch.proto"])
            .expect("the scratch proto compiles");
        let pool = compiler.descriptor_pool();
        let service = pool
            .get_service_by_name("origincloud.scratch.v1.Scratch")
            .expect("the service is in the pool");
        let declared: Vec<(String, bool)> = service
            .methods()
            .map(|m| (m.name().to_owned(), declared_idempotent(&m)))
            .collect();
        assert_eq!(
            declared,
            [
                ("Wait".to_owned(), true),
                ("Set".to_owned(), true),
                ("Change".to_owned(), false)
            ]
        );
    }
}
