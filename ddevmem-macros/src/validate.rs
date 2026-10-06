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

use crate::ast::{Bitfield, EnumDef, FieldAccess, FieldType, RegisterEntry, RegisterMap};

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
        self.claim_hinted(name, span, "", sink);
    }

    /// Like [`claim`](Self::claim), appending `hint` to the collision error.
    fn claim_hinted(&mut self, name: String, span: Span, hint: &str, sink: &mut ErrorSink) {
        if self.taken.insert(name.clone(), span).is_some() {
            sink.error(
                span,
                format!(
                    "this declaration generates a {} named `{}`, which an earlier \
                     declaration in the same map also generates{hint}",
                    self.kind, name
                ),
            );
        }
    }
}

/// Largest value a field of `bits` bits can hold.
fn max_value(bits: u64) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    }
}

pub fn validate(map: &RegisterMap) -> syn::Result<()> {
    let mut sink = ErrorSink::default();
    let mut methods = Namespace::new("method");
    let mut types = Namespace::new("type");
    let bus_size = map.bus.kind.size_bytes();

    for def in &map.enums {
        validate_enum(def, &mut types, &mut sink);
    }
    for entry in &map.entries {
        validate_entry(map, entry, bus_size, &mut methods, &mut types, &mut sink);
    }

    sink.finish()
}

/// Checks an enum declaration on its own; whether its values fit each field
/// that uses it is checked per field.
fn validate_enum(def: &EnumDef, types: &mut Namespace, sink: &mut ErrorSink) {
    types.claim_hinted(
        def.name.to_string(),
        def.name.span(),
        "; to share one enum between fields, declare it once and refer to it \
         by name (`as Name`)",
        sink,
    );
    if def.variants.is_empty() {
        sink.error(
            def.name.span(),
            format!("enum `{}` must declare at least one variant", def.name),
        );
    }

    let mut names = Namespace::new("variant");
    let mut values: HashMap<u64, Span> = HashMap::new();
    for variant in &def.variants {
        names.claim(variant.name.to_string(), variant.name.span(), sink);
        let Some(value) = variant.value.value else {
            continue;
        };
        // An inline enum's raw type is its register's, which the field-width
        // check already covers with a more precise message.
        if let (true, Some(bits)) = (def.standalone, def.raw.kind.width_bits()) {
            if value > max_value(u64::from(bits)) {
                sink.error(
                    variant.value.span(),
                    format!(
                        "variant value {value:#x} does not fit in `{}`, the map's bus \
                         type and the raw type of enum `{}`",
                        def.raw.ident, def.name
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
    // `wo` registers with bitfields get a whole-register builder.
    if entry.access == crate::ast::Access::Wo && !entry.bitfields.is_empty() {
        methods.claim(format!("write_{reg}"), reg_span, sink);
        types.claim(crate::expand::writer_type_name(map, entry), reg_span, sink);
    }

    for bf in &entry.bitfields {
        validate_bitfield(map, entry, bf, methods, sink);
    }
}

fn validate_bitfield(
    map: &RegisterMap,
    entry: &RegisterEntry,
    bf: &Bitfield,
    methods: &mut Namespace,
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

    // A field may narrow the register's access, never widen it.
    match bf.access {
        FieldAccess::Inherit => {}
        FieldAccess::Ro if !entry.access.can_read() => sink.error(
            field_span,
            format!(
                "`ro` field `{field}` cannot appear in the write-only register `{reg}`"
            ),
        ),
        FieldAccess::Wo if !entry.access.can_write() => sink.error(
            field_span,
            format!(
                "`wo` field `{field}` cannot appear in the read-only register `{reg}`"
            ),
        ),
        FieldAccess::W1c if !entry.access.can_write() => sink.error(
            field_span,
            format!(
                "`w1c` field `{field}` cannot appear in the read-only register `{reg}`: \
                 clearing it requires a write"
            ),
        ),
        _ => {}
    }

    if bf.access.can_read(entry.access) {
        methods.claim(format!("{reg}_{field}"), field_span, sink);
    }
    if bf.access.can_write(entry.access) {
        let setter = match bf.access {
            FieldAccess::W1c => format!("clear_{reg}_{field}"),
            _ => format!("set_{reg}_{field}"),
        };
        methods.claim(setter, field_span, sink);
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
        FieldType::Enum(enum_use) => {
            let Some(width) = width else {
                return;
            };
            let def = &map.enums[enum_use.index];
            let raw_max = def
                .raw
                .kind
                .width_bits()
                .map(|bits| max_value(u64::from(bits)));
            for variant in &def.variants {
                let Some(value) = variant.value.value else {
                    continue;
                };
                // Already reported against the declaration.
                if def.standalone && raw_max.is_some_and(|max| value > max) {
                    continue;
                }
                if value <= max_value(width) {
                    continue;
                }
                if enum_use.inline {
                    sink.error(
                        variant.value.span(),
                        format!(
                            "variant value {value:#x} does not fit in the {width}-bit \
                             field `{field}`"
                        ),
                    );
                } else {
                    sink.error(
                        enum_use.span,
                        format!(
                            "variant `{}` of enum `{}` has the value {value:#x}, which \
                             does not fit in the {width}-bit field `{field}`",
                            variant.name, def.name
                        ),
                    );
                }
            }
        }
        // The type is only known to the compiler: `FieldValue` is enforced
        // by the generated code, and the setter masks whatever it returns.
        FieldType::Custom(_) => {}
    }
}
