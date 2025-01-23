use proc_macro2::Span;
use quote::{quote, ToTokens};
use std::collections::HashMap;
use syn::{
    parse_quote,
    visit_mut::{visit_file_mut, VisitMut},
    Attribute, File, ForeignItem, Ident, Item, ItemConst, ItemEnum, ItemFn,
    ItemForeignMod, ItemImpl, ItemMod, ItemStatic, ItemStruct, ItemType,
    ItemUnion, ItemUse,
};

use crate::HashSet;

pub fn merge_cfg_attributes(file: &mut File) {
    let mut visitor = Visitor;
    visitor.visit_file_mut(file);
}

struct Visitor;

impl VisitMut for Visitor {
    fn visit_file_mut(&mut self, file: &mut File) {
        process_items(&mut file.items);
    }
}

#[derive(Default)]
struct AttributeSet {
    cfg_attrs: HashSet<Attribute>,
    cc_attrs: HashSet<Attribute>,
    other_attrs: HashSet<Attribute>,
}
use itertools::Itertools;

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

fn process_items(items: &mut Vec<Item>) {
    let mut synthetic_mods: HashMap<Ident, (AttributeSet, Vec<Item>)> =
        HashMap::new();
    let mut new_items = Vec::new();

    for mut item in std::mem::take(items) {
        match &mut item {
            Item::Const(ItemConst { ref mut attrs, .. }) |
            Item::Struct(ItemStruct { ref mut attrs, .. }) |
            Item::Enum(ItemEnum { ref mut attrs, .. }) |
            Item::Fn(ItemFn { ref mut attrs, .. }) |
            Item::Union(ItemUnion { ref mut attrs, .. }) |
            Item::Type(ItemType { ref mut attrs, .. }) |
            Item::Impl(ItemImpl { ref mut attrs, .. }) |
            Item::Mod(ItemMod { ref mut attrs, .. }) |
            Item::Use(ItemUse { ref mut attrs, .. }) |
            Item::Static(ItemStatic { ref mut attrs, .. }) => {
                let attr_set = partition_attributes(attrs);
                *attrs = attr_set.other_attrs.iter().cloned().collect();

                let items = if !attr_set.cfg_attrs.is_empty() || !attr_set.cc_attrs.is_empty() {
                    &mut synthetic_mods
                        .entry(attr_set.ident())
                        .or_insert_with(|| (attr_set, vec![]))
                        .1
                } else {
                    &mut new_items
                };
                items.push(item);
            }

            Item::ForeignMod(ItemForeignMod {
                ref mut attrs,
                ref mut items,
                ..
            }) => {
                for foreign_item in items.iter_mut() {
                    let mut attr_set = partition_attributes(&attrs);
                    let inner_attrs = match foreign_item {
                        ForeignItem::Fn(ref mut foreign_fn) => {
                            &mut foreign_fn.attrs
                        }
                        ForeignItem::Static(ref mut foreign_static) => {
                            &mut foreign_static.attrs
                        }
                        _ => &mut vec![],
                    };

                    let inner_attr_set = partition_attributes(inner_attrs);
                    attr_set
                        .other_attrs
                        .extend(inner_attr_set.other_attrs.clone());
                    attr_set.cfg_attrs.extend(inner_attr_set.cfg_attrs);
                    attr_set.cc_attrs.extend(inner_attr_set.cc_attrs);
                    *inner_attrs =
                        inner_attr_set.other_attrs.into_iter().collect();

                    let items = if !attr_set.cfg_attrs.is_empty() || !attr_set.cc_attrs.is_empty() {
                        &mut synthetic_mods
                            .entry(attr_set.ident())
                            .or_insert_with(|| (attr_set, vec![]))
                            .1
                    } else {
                        &mut new_items
                    };
                    items.push(Item::Verbatim(quote! {
                        #foreign_item
                    }));
                }
            }
            _ => {
                new_items.push(item);
            }
        }
    }

    for (ident, (attr_set, items)) in synthetic_mods {
        let cfg_attrs: Vec<_> = attr_set.cfg_attrs.iter().collect();
        let cc_attrs: Vec<_> = attr_set.cc_attrs.iter().collect();
        let block = if cc_attrs.is_empty() {
            quote! {
                #(#items)*
            }
        } else {
            // TODO: include unsafe and abi from original items
            quote! {
                #(#cc_attrs)*
                unsafe extern "C" {
                    #(#items)*
                }
            }
        };

        new_items.push(Item::Verbatim(quote! {
            #(#cfg_attrs)*
            pub mod #ident {
                #block
            }

            #(#cfg_attrs)*
            pub use #ident::*;
        }));
    }

    items.extend(new_items);
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
