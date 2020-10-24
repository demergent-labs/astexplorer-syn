use quote::ToTokens;
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;

mod types {
    use indexmap::IndexMap;
    use proc_macro2::TokenStream;
    use quote::{quote, ToTokens, TokenStreamExt};
    use serde::{Deserialize, Deserializer};

    // Manual blacklist for now. See https://github.com/dtolnay/syn/issues/607#issuecomment-475905135.
    fn has_spanned(ty: &str) -> bool {
        match ty {
            "DataStruct" | "DataEnum" | "DataUnion" => false,
            "FnDecl" => false,
            "QSelf" => false,
            _ => true,
        }
    }

    #[derive(Debug, PartialEq, Eq, Hash, Deserialize)]
    pub struct Ident(String);

    impl ToTokens for Ident {
        fn to_tokens(&self, tokens: &mut TokenStream) {
            proc_macro2::Ident::new(&self.0, proc_macro2::Span::call_site()).to_tokens(tokens)
        }
    }

    #[derive(Debug, PartialEq, Deserialize)]
    pub struct Definitions {
        pub types: Vec<Node>,
        pub tokens: IndexMap<Ident, String>,
    }

    impl ToTokens for Definitions {
        fn to_tokens(&self, tokens: &mut TokenStream) {
            tokens.append_all(&self.types);
            for key in self.tokens.keys() {
                tokens.append_all(quote! {
                    impl ToJS for syn::token::#key {
                        fn to_js(&self) -> JsValue {
                            js!(#key {
                                span: self.span()
                            })
                        }
                    }
                });
            }
        }
    }

    #[derive(Debug, PartialEq, Deserialize)]
    pub struct Node {
        pub ident: Ident,
        #[serde(flatten, deserialize_with = "private_if_absent")]
        pub data: Data,
    }

    impl ToTokens for Node {
        fn to_tokens(&self, tokens: &mut TokenStream) {
            let ident = &self.ident;

            let data = match &self.data {
                Data::Private => {
                    quote! {
                        js!(#ident {
                            value: self.value(),
                            span: self.span()
                        })
                    }
                }
                Data::Struct(fields) => {
                    let mut fields = fields.iter().collect::<Vec<_>>();

                    // Move groups down or they will be the target of any locations.
                    fields.sort_by_key(|(_field, ty)| match ty {
                        Type::Group(_) => 1,
                        Type::Syn(ident) if ident.0 == "MacroDelimiter" => 1,
                        _ => 0,
                    });

                    let fields = fields
                        .into_iter()
                        .map(|(field, _ty)| {
                            quote! {
                                #field: self.#field
                            }
                        })
                        .chain(if has_spanned(&ident.0) {
                            Some(quote! {
                                span: self.span()
                            })
                        } else {
                            None
                        });

                    quote! {
                        js!(#ident {
                            #(#fields,)*
                        })
                    }
                }
                Data::Enum(variants) => {
                    let matches = variants.iter().map(|(variant, types)| {
                        let variant = quote! {
                            #ident::#variant
                        };

                        let variant_path = quote! {
                            syn::#variant
                        };

                        match types.len() {
                            0 => quote! {
                               #variant_path => js!(#variant {})
                            },
                            1 => quote! {
                               #variant_path(x) => x.to_js()
                            },
                            _ => {
                                let payload = (0..types.len()).map(|i| Ident(format!("x{}", i)));
                                let payload = quote! { #(#payload),* };

                                quote! {
                                    #variant_path(#payload) => js!(#variant { span: self.span() } [#payload])
                                }
                            }
                        }
                    });
                    quote! {
                        match self {
                            #(#matches,)*
                        }
                    }
                }
            };

            tokens.append_all(quote! {
                impl ToJS for syn::#ident {
                    fn to_js(&self) -> JsValue {
                        #data
                    }
                }
            });
        }
    }

    #[derive(Debug, PartialEq, Deserialize)]
    pub enum Data {
        Private,
        #[serde(rename = "fields")]
        Struct(Fields),
        #[serde(rename = "variants")]
        Enum(Variants),
    }

    pub type Fields = IndexMap<Ident, Type>;
    pub type Variants = IndexMap<Ident, Vec<Type>>;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum Type {
        /// Type defined by `syn`
        Syn(Ident),

        /// Type defined in `std`.
        Std(Ident),

        /// Type external to `syn`
        #[serde(rename = "proc_macro2")]
        Ext(Ident),

        /// Token type
        Token(Ident),

        /// Token group
        Group(Ident),

        /// Punctuated list
        Punctuated(Punctuated),

        Option(Box<Type>),
        Box(Box<Type>),
        Vec(Box<Type>),
        Tuple(Vec<Type>),
    }

    #[derive(Debug, PartialEq, Deserialize)]
    pub struct Punctuated {
        pub element: Box<Type>,
        pub punct: String,
    }

    fn private_if_absent<'de, D>(deserializer: D) -> Result<Data, D::Error>
    where
        D: Deserializer<'de>,
    {
        let option = Option::deserialize(deserializer)?;
        Ok(option.unwrap_or(Data::Private))
    }
}

fn main() {
    let body: types::Definitions = serde_json::from_str(include_str!("syn/syn.json")).unwrap();

    let generated = body.into_token_stream();

    let path = &Path::new(&env::var_os("OUT_DIR").unwrap()).join("to_js.rs");

    {
        let mut out = File::create(path).unwrap();
        writeln!(out, "{}", generated).unwrap();
    }

    let _ = std::process::Command::new("rustfmt").arg(path).status();
}
