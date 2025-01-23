use proc_macro2::Span;
use quote::{quote, ToTokens};
use syn::{
    Attribute, File, ForeignItem, Ident, Item, ItemConst, ItemEnum, ItemFn, ItemForeignMod,
    ItemImpl, ItemMod, ItemStatic, ItemStruct, ItemType, ItemUnion, ItemUse,
};
use itertools::Itertools;
use crate::HashMap;
use crate::HashSet;

pub fn merge_cfg_attributes(file: &mut File) {
    let mut visitor = Visitor::new();
    visitor.visit_file(file);
}

struct Visitor {
    synthetic_mods: HashMap<Ident, (AttributeSet, Vec<Item>)>,
    new_items: Vec<Item>,
}

impl Visitor {
    fn new() -> Self {
        Self {
            synthetic_mods: HashMap::default(),
            new_items: Vec::new(),
        }
    }

    fn visit_file(&mut self, file: &mut File) {
        self.visit_items(&mut file.items);

        for (ident, (attr_set, items)) in self.synthetic_mods.drain() {
            let cfg_attrs: Vec<_> = attr_set.cfg_attrs.iter().collect();
            let cc_attrs: Vec<_> = attr_set.cc_attrs.iter().collect();
            let block = if cc_attrs.is_empty() {
                quote! {
                    #(#items)*
                }
            } else {
                quote! {
                    #(#cc_attrs)*
                    unsafe extern "C" {
                        #(#items)*
                    }
                }
            };

            self.new_items.push(Item::Verbatim(quote! {
                #(#cfg_attrs)*
                pub mod #ident {
                    #block
                }

                #(#cfg_attrs)*
                pub use #ident::*;
            }));
        }

        file.items = std::mem::take(&mut self.new_items);
    }

    fn visit_items(&mut self, items: &mut Vec<Item>) {
        for mut item in std::mem::take(items) {
            match &mut item {
                Item::Const(ItemConst { ref mut attrs, .. })
                | Item::Struct(ItemStruct { ref mut attrs, .. })
                | Item::Enum(ItemEnum { ref mut attrs, .. })
                | Item::Fn(ItemFn { ref mut attrs, .. })
                | Item::Union(ItemUnion { ref mut attrs, .. })
                | Item::Type(ItemType { ref mut attrs, .. })
                | Item::Impl(ItemImpl { ref mut attrs, .. })
                | Item::Mod(ItemMod { ref mut attrs, .. })
                | Item::Use(ItemUse { ref mut attrs, .. })
                | Item::Static(ItemStatic { ref mut attrs, .. }) => {
                    let attr_set = partition_attributes(attrs);
                    *attrs = attr_set.other_attrs.iter().cloned().collect();
                    self.insert_item_into_mod(attr_set, item);
                }
                Item::ForeignMod(foreign_mod) => {
                    self.visit_foreign_mod(foreign_mod);
                }
                _ => {
                    self.new_items.push(item);
                }
            }
        }
    }

    fn visit_foreign_mod(&mut self, foreign_mod: &mut ItemForeignMod) {
        for mut foreign_item in std::mem::take(&mut foreign_mod.items) {
            let mut attr_set = partition_attributes(&foreign_mod.attrs);
            let inner_attrs = match &mut foreign_item {
                ForeignItem::Fn(f) => &mut f.attrs,
                ForeignItem::Static(s) => &mut s.attrs,
                ForeignItem::Type(t) => &mut t.attrs,
                ForeignItem::Macro(m) => &mut m.attrs,
                _ => &mut Vec::new(),
            };

            let inner_attr_set = partition_attributes(inner_attrs);
            attr_set.other_attrs.extend(inner_attr_set.other_attrs);
            attr_set.cfg_attrs.extend(inner_attr_set.cfg_attrs);
            attr_set.cc_attrs.extend(inner_attr_set.cc_attrs);
            *inner_attrs = attr_set.other_attrs.iter().cloned().collect();

            self.insert_item_into_mod(
                attr_set,
                Item::Verbatim(quote! { #foreign_item }),
            );
        }
    }

    fn insert_item_into_mod(&mut self, attr_set: AttributeSet, item: Item) {
        if !attr_set.cfg_attrs.is_empty() || !attr_set.cc_attrs.is_empty() {
            let (_, items) = self.synthetic_mods
                .entry(attr_set.ident())
                .or_insert_with(|| (attr_set, Vec::new()));
            items.push(item);
        } else {
            self.new_items.push(item);
        }
    }
}

#[derive(Default)]
struct AttributeSet {
    cfg_attrs: HashSet<Attribute>,
    cc_attrs: HashSet<Attribute>,
    other_attrs: HashSet<Attribute>,
}

impl AttributeSet {
    fn ident(&self) -> Ident {
        assert!(!self.cfg_attrs.is_empty() || !self.cc_attrs.is_empty());

        Ident::new(
            self.cfg_attrs
                .iter()
                .chain(self.cc_attrs.iter())
                .map(|attr| attr.to_token_stream().to_string())
                .sorted()
                .map(|s| {
                    s.replace('=', "_eq_").replace(
                        |c: char| !c.is_alphanumeric() && c != '_',
                        "_",
                    )
                })
                .join("_")
                .chars()
                .coalesce(|a, b| {
                    if a == '_' && b == '_' {
                        Ok(a)
                    } else {
                        Err((a, b))
                    }
                })
                .collect::<String>()
                .trim_matches('_'),
            Span::call_site(),
        )
    }
}

fn partition_attributes(attrs: &[Attribute]) -> AttributeSet {
    let mut attribute_set = AttributeSet::default();

    for attr in attrs {
        let target_set = if let Some(ident) = attr.path().get_ident() {
            match ident.to_string().as_str() {
                "cfg" => &mut attribute_set.cfg_attrs,
                "link" => &mut attribute_set.cc_attrs,
                _ => &mut attribute_set.other_attrs,
            }
        } else {
            &mut attribute_set.other_attrs
        };
        target_set.insert(attr.clone());
    }

    attribute_set
}
