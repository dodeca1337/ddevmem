//! Semantic validation of a parsed [`RegisterMap`].
//!
//! Everything that can be proven wrong at macro-expansion time is reported
//! here, with the span of the offending token — long before the generated
//! code would produce an inscrutable type error. Checks that depend on the
//! target (anything involving `usize`) are deferred to `const` assertions
//! emitted by the code generator.

use std::collections::HashMap;
use std::fmt::Display;

use proc_macro2::Span;

use crate::ast::{Bitfield, FieldType, RegisterEntry, RegisterMap};

#[derive(Default)]
struct ErrorSink {
    errors: Vec<syn::Error>,
}

impl ErrorSink {
    fn error(&mut self, span: Span, message: impl Display) {
        self.errors.push(syn::Error::new(span, message));
    }

    fn finish(self) -> syn::Result<()> {
        let mut iter = self.errors.into_iter();
        let Some(mut combined) = iter.next() else {
            return Ok(());
        };
        for error in iter {
            combined.combine(error);
        }
        Err(combined)
    }
}

/// Tracks names that will exist in one namespace of the generated code
/// (methods of the map struct, or module-level types) and reports collisions.
#[derive(Default)]
struct Namespace {
    kind: &'static str,
    taken: HashMap<String, Span>,
}

impl Namespace {
    fn new(kind: &'static str) -> Self {
        Self {
            kind,
            taken: HashMap::new(),
        }
    }

    fn claim(&mut self, name: String, span: Span, sink: &mut ErrorSink) {
        if self.taken.insert(name.clone(), span).is_some() {
            sink.error(
                span,
                format!(
                    "this declaration generates a {} named `{}`, which an earlier \
                     declaration in the same map also generates",
                    self.kind, name
                ),
            );
        }
    }
}

pub fn validate(map: &RegisterMap) -> syn::Result<()> {
    let mut sink = ErrorSink::default();
    let mut methods = Namespace::new("method");
    let mut types = Namespace::new("type");
    let bus_size = map.bus.kind.size_bytes();

    for entry in &map.entries {
        validate_entry(map, entry, bus_size, &mut methods, &mut types, &mut sink);
    }

    sink.finish()
}

fn validate_entry(
    map: &RegisterMap,
    entry: &RegisterEntry,
    bus_size: Option<u64>,
    methods: &mut Namespace,
    types: &mut Namespace,
    sink: &mut ErrorSink,
) {
    let reg = entry.name.to_string();
    let reg_span = entry.name.span();

    if reg.starts_with("__") {
        sink.error(
            reg_span,
            "register names starting with `__` are reserved for macro internals",
        );
    }
    if reg == "new" {
        sink.error(
            reg_span,
            "a register cannot be named `new`: it would collide with the generated constructor",
        );
    }

    // Register type must fit into a single bus access.
    if let (Some(ty), Some(bus)) = (entry.ty.kind.size_bytes(), bus_size) {
        if ty > bus {
            sink.error(
                entry.ty.span(),
                format!(
                    "register type is wider than the `{}` bus ({} > {} bytes)",
                    map.bus.ident, ty, bus
                ),
            );
        }
    }

    // Every register occupies a full bus slot, so its offset must be
    // bus-aligned.
    if let (Some(offset), Some(bus)) = (entry.offset.value, bus_size) {
        if offset % bus != 0 {
            sink.error(
                entry.offset.span(),
                format!(
                    "offset {offset:#x} is not aligned to the {bus}-byte bus width"
                ),
            );
        }
    }

    if entry.array_len.as_ref().and_then(|len| len.value) == Some(0) {
        sink.error(
            entry.array_len.as_ref().unwrap().span(),
            "register array length must be at least 1",
        );
    }

    // Claim every method this entry will generate.
    methods.claim(format!("{reg}_offset"), reg_span, sink);
    methods.claim(format!("{reg}_address"), reg_span, sink);
    if entry.is_array() {
        methods.claim(format!("{reg}_len"), reg_span, sink);
    }
    if entry.access.can_read() {
        methods.claim(reg.clone(), reg_span, sink);
    }
    if entry.access.can_write() {
        methods.claim(format!("set_{reg}"), reg_span, sink);
    }
    if entry.access.can_read() && entry.access.can_write() {
        methods.claim(format!("modify_{reg}"), reg_span, sink);
    }

    for bf in &entry.bitfields {
        validate_bitfield(entry, bf, methods, types, sink);
    }
}

fn validate_bitfield(
    entry: &RegisterEntry,
    bf: &Bitfield,
    methods: &mut Namespace,
    types: &mut Namespace,
    sink: &mut ErrorSink,
) {
    let reg = entry.name.to_string();
    let field = bf.name.to_string();
    let field_span = bf.name.span();

    if field.starts_with("__") {
        sink.error(
            field_span,
            "bitfield names starting with `__` are reserved for macro internals",
        );
    }

    if entry.access.can_read() {
        methods.claim(format!("{reg}_{field}"), field_span, sink);
    }
    if entry.access.can_write() {
        methods.claim(format!("set_{reg}_{field}"), field_span, sink);
    }

    // Bit range sanity, when the positions are literals.
    let ty_bits = entry.ty.kind.width_bits();
    if let (Some(lo), Some(hi)) = (bf.lo.value, bf.hi.value) {
        if hi < lo {
            sink.error(
                bf.hi.span(),
                format!("bitfield `{field}`: high bit {hi} is below low bit {lo}"),
            );
            return;
        }
        if let Some(bits) = ty_bits {
            if hi >= u64::from(bits) {
                sink.error(
                    bf.hi.span(),
                    format!(
                        "bitfield `{field}`: bit {hi} does not exist in the {bits}-bit \
                         register type"
                    ),
                );
                return;
            }
        }
    }
    let width = match (bf.lo.value, bf.hi.value) {
        (Some(lo), Some(hi)) => Some(hi - lo + 1),
        _ => None,
    };

    match &bf.ty {
        FieldType::Raw => {}
        FieldType::Bool => {
            if let Some(width) = width {
                if width != 1 {
                    sink.error(
                        field_span,
                        format!("`as bool` requires a single-bit field, but `{field}` spans {width} bits"),
                    );
                }
            }
        }
        FieldType::Int(cast) => {
            if let (Some(width), Some(cast_bits)) = (width, cast.kind.width_bits()) {
                if u64::from(cast_bits) < width {
                    sink.error(
                        cast.span(),
                        format!(
                            "cast type `{}` is narrower than the {width}-bit field `{field}`",
                            cast.ident
                        ),
                    );
                }
            }
        }
        FieldType::Enum(def) => {
            types.claim(def.name.to_string(), def.name.span(), sink);
            if def.variants.is_empty() {
                sink.error(
                    def.name.span(),
                    "an `as enum` bitfield must declare at least one variant",
                );
            }

            let mut names = Namespace::new("variant");
            let mut values: HashMap<u64, Span> = HashMap::new();
            for variant in &def.variants {
                names.claim(variant.name.to_string(), variant.name.span(), sink);
                let Some(value) = variant.value.value else {
                    continue;
                };
                if let Some(width) = width {
                    // width <= 64 is guaranteed by the range checks above.
                    let max = u64::MAX >> (64 - width);
                    if value > max {
                        sink.error(
                            variant.value.span(),
                            format!(
                                "variant value {value:#x} does not fit in the {width}-bit \
                                 field `{field}`"
                            ),
                        );
                    }
                }
                if values.insert(value, variant.value.span()).is_some() {
                    sink.error(
                        variant.value.span(),
                        format!("duplicate variant value {value:#x} in enum `{}`", def.name),
                    );
                }
            }
        }
    }
}
