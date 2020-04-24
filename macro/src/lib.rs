extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::Span;
use syn::{parse_macro_input,  Fields, ItemStruct, Ident, FieldsNamed, ExprLit, Lit, LitInt};
use quote::quote;

#[proc_macro]
pub fn generate_storage_ty(input: TokenStream) -> TokenStream {
    let i = parse_macro_input!(input as ItemStruct);

    //eprint!("{:#?}", &i);

    let ty_name_str = i.ident.to_string();

    let ty_name = Ident::new(&ty_name_str, Span::call_site());
    let _un_ty_name = Ident::new(&format!("Recast{}", &ty_name_str), Span::call_site());

    let fields = if let ItemStruct { fields : Fields::Named( FieldsNamed{ named, .. } ), .. } = &i {
        named
    } else {
        unimplemented!()
    };
    //eprint!("fields : {:#?}", &fields);

    let field_name : Vec<&_> = fields.into_iter().filter_map(|f| {
        f.ident.as_ref()
    }).collect();

    let field_ty : Vec<&_> = fields.into_iter().map(|f| {
        &f.ty
    }).collect();

    let setter_names : Vec<_> = (&field_name).into_iter().map(|name| {
        Ident::new(&format!("set_{}", name.to_string()), name.span())
    }).collect();

    let getter_names : Vec<_> = (&field_name).into_iter().map(|name| {
        Ident::new(&format!("get_{}", name.to_string()), name.span())
    }).collect();

    let _tail_names : Vec<_> = (&field_name).into_iter().map(|name| {
        Ident::new(&format!("pos_{}", name.to_string()), name.span())
    }).collect();
    
    let uids : Vec<_> = (0 .. field_name.len()).into_iter().map(|num| {
        ExprLit {
            attrs : vec![],
            lit : Lit::Int(LitInt::new(&num.to_string() , Span::call_site())),
        }
    }).collect();


    let max_recods_num = ExprLit {
        attrs : vec![],
        lit : Lit::Int(LitInt::new(&fields.len().to_string() , Span::call_site())),
    };


    let out = quote!(
        //use $crate::Record;


        const MAX_RECORD_SZ : usize = 0x80;
        const MAX_RECORDS_NUMBER : usize = #max_recods_num;

        //union #un_ty_name {
        //    #( #field_name : #field_ty, )*
        //    buf : [u8;VALUE_MAX_SZ],
        //}

        pub struct #ty_name<M> {
            storage      : Storage<M>,
            record_table : [RecordDesc; MAX_RECORDS_NUMBER],
        }

        impl<M : StorageMem> #ty_name<M> {
            pub fn new(mem : M) -> Self {
                Self {
                    storage : Storage::new(mem),
                    record_table : [
                        #(RecordDesc {
                            tag : #uids,
                            ptr : None,
                        }),*
                    ],
                }
            }

            pub fn init(&mut self, hasher : &mut impl StorageHasher32) -> InitStats {
                self.storage.init(&mut self.record_table, hasher)
            }

            #( 
                pub fn #getter_names(&self) ->  Result<Option<&'static #field_ty>, Error> {
                    let record_desc = &self.record_table[#uids];
                    let some = self.storage.get(record_desc)?;
                    
                    match some {
                        Some(payload) => {
                            unsafe {
                                let field_ptr = payload.as_ptr() as usize as *const #field_ty;
                                Ok(Some(&*field_ptr))
                                //Ok(Some(&0))
                            }
                        }
                        None => Ok(None),
                    }
                }
            )*

            #( 
                pub fn #setter_names(&mut self, #field_name : #field_ty, hasher : &mut impl StorageHasher32) -> Result<(),Error> {
                    let mut record_desc = &mut self.record_table[#uids];
                    
                    let field_ptr : *const Word = (&#field_name) as *const _ as usize as *const Word;
                    const FILED_SIZE : usize = ::core::mem::size_of::<#field_ty>();
                    let payload_slice_sz : usize = match FILED_SIZE {
                        0 => 0,
                        n => {
                            if n >= WORD_SIZE {
                                n / WORD_SIZE
                            } else {
                                WORD_SIZE
                            }
                        }
                    };
                    let src_words : &[Word] = unsafe { ::core::slice::from_raw_parts(field_ptr, payload_slice_sz) };
                    self.storage.update(record_desc, src_words, hasher)
                }
            )*
        }

        impl<M : StorageMem> ::core::fmt::Debug for #ty_name<M> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    write!(f, "{} {{\n", stringify!(#ty_name))?;
                    #( 
                        let name_str = stringify!( #field_name );
                        let value = self.#getter_names();
                        write!(f, "    {} : {:?}\n", name_str, value)?;
                    )*
                    write!(f, "}}\n")
            }
        }
    );

    TokenStream::from(out)
}


