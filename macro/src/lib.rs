extern crate proc_macro;

use proc_macro2::{Span, TokenStream};
use syn::{
    parse_macro_input,
    Fields,
    ItemStruct,
    Ident,
    FieldsNamed,
    Field,
    ExprLit,
    Lit,
    LitInt,
    Type,
    TypeReference,
    Lifetime,
    TypePath,
    Path,
    PathSegment,
    TypeSlice,
};
use quote::quote;

#[proc_macro]
pub fn generate_storage_ty(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let i = parse_macro_input!(input as ItemStruct);

    //eprint!("{:#?}", &i);

    let ty_name_str = i.ident.to_string();

    let ty_name = Ident::new(&ty_name_str, Span::call_site());
    let _un_ty_name = Ident::new(&format!("Recast{}", &ty_name_str), Span::call_site());

    let fields = if let ItemStruct { fields : Fields::Named( FieldsNamed{ named, .. } ), .. } = &i {
        named
    } else {
        unimplemented!("Only structs supported")
    };
    //eprint!("fields : {:#?}", &fields);
    let setters_getters = setters_getters(
        &fields.iter().enumerate().map(|(uid, f)|{
            (
                f.clone(),
                LitInt::new(&(uid + 1).to_string(),Span::call_site()),
            )
        })
        .collect::<Vec<_>>()
    );

    let field_name : Vec<&_> = fields.into_iter().filter_map(|f| {
        f.ident.as_ref()
    }).collect();

    let _field_ty : Vec<&_> = fields.into_iter().map(|f| {
        &f.ty
    }).collect();

    let _setter_names : Vec<_> = (&field_name).into_iter().map(|name| {
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
            lit : Lit::Int(LitInt::new(&(num + 1).to_string() , Span::call_site())),
        }
    }).collect();


    let max_recods_num = ExprLit {
        attrs : vec![],
        lit : Lit::Int(LitInt::new(&fields.len().to_string() , Span::call_site())),
    };


    let out = quote!(

        //const MAX_RECORD_SZ : usize = 0x80;
        //const MAX_RECORDS_NUMBER : usize = #max_recods_num + 1;

        pub struct #ty_name<M, H> {
            storage: Storage<M, H>,
            record_table: [RecordDesc; #max_recods_num + 1],
        }

        impl<M, H> #ty_name<M, H> 
        where 
            M: StorageMem,
            M::Error: ::core::fmt::Debug,
            H: StorageHasher32,
        {
            pub fn new(mem: M) -> Self {
                Self {
                    storage: Storage::<M, H>::new(mem),
                    record_table: [
                        RecordDesc {
                            tag: 0,
                            ptr: None,
                        },
                        #(RecordDesc {
                            tag: #uids,
                            ptr: None,
                        }),*
                    ],
                }
            }

            pub fn init(&mut self, hasher: &mut H) -> InitStats {
                self.storage.init(&mut self.record_table, hasher)
            }

            #setters_getters
        }

        impl<M, H> ::core::fmt::Debug for #ty_name<M, H>
        where 
            M: StorageMem,
            M::Error: ::core::fmt::Debug,
            H: StorageHasher32,
        {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    write!(f, "{} {{\n", stringify!(#ty_name))?;
                    #( 
                        let name_str = stringify!( #field_name );
                        let value = self.#getter_names(None).unwrap();
                        write!(f, "    {} : {:?}\n", name_str, value)?;
                    )*
                    write!(f, "}}\n")
            }
        }
    );

    proc_macro::TokenStream::from(out)
}

fn setters_getters(fields: &Vec<(Field, LitInt)>) -> TokenStream {
    let mut tt = TokenStream::new();
    for (f, uid) in fields {
        match f {
            // Matching &'static types
            Field{
                ident: Some(ident_name),
                ty: Type::Reference(TypeReference{
                    elem, 
                    mutability: None,
                    lifetime: Some(Lifetime{ident: lf_ident, ..}),
                    ..
                }),
                ..
            } => {
                let nested_ty = &**elem;
                if *lf_ident == "static" {
                    match nested_ty {
                        Type::Path(TypePath{path: Path{segments, ..}, ..})  => {
                            let PathSegment{ident, ..} = &segments
                                .first()
                                .expect("Unsupported strange type behind ref");

                            if *ident == "str" {
                                let sg = setter_getter_static_str(ident_name, uid);
                                tt.extend(sg);
                            } else {
                               unimplemented!("Unsupported field type behind reference")
                            }
                        }
                        Type::Slice(TypeSlice{elem, ..}) => {
                            let nested_ty = &**elem;
                            if let Type::Path(TypePath{path: Path{segments, ..}, ..}) = nested_ty {
                                let PathSegment{ident, ..} = &segments
                                    .first()
                                    .expect("Unsupported strange type behind ref");

                                if *ident == "u8" {
                                    let sg = setter_getter_static_byte_slice(ident_name, uid);
                                    tt.extend(sg);
                                } else {
                                   unimplemented!("Unsupported field type behind reference")
                                }
                            } else {
                               unimplemented!("Unsupported field type slice behind reference")
                            }
                        }
                        _ => unimplemented!("Unsupported field type behind reference")
                    }
                } else {
                    unimplemented!("Only supproted 'static ref types")
                }
            }
            // Matching primitive and composite types
            Field{ident: Some(ident_name), ty: Type::Path(TypePath{path: Path{segments, ..}, ..}), ..} =>  {
                let PathSegment{ident: ty, ..} = &segments
                    .first()
                    .expect("Unsupported strange type behind ref");
                
                let sg = setter_getter_primitive_composite(ident_name, ty, uid);
                tt.extend(sg);
            }

            _ => unimplemented!("Unsupported field type"),
        }
    }

    tt
}

fn setter_getter_primitive_composite(name: &Ident, ty: &Ident, uid: &LitInt) -> TokenStream {
    let setter_name = Ident::new(&("set_".to_string() + &name.to_string()), Span::call_site());
    let getter_name = Ident::new(&("get_".to_string() + &name.to_string()), Span::call_site());
    quote!(
        pub fn #setter_name(&mut self, #name: #ty, hasher: &mut H)
            -> Result<(),Error<M::Error>>
        {

            assert!(::core::mem::align_of::<#ty>() <= ::core::mem::align_of::<Word>(), "Aligment of type to big");

            let mut record_desc = &mut self.record_table[#uid];

            let src = unsafe { 
                ::core::slice::from_raw_parts(
                     (&#name) as *const _ as usize as *const u8,
                     ::core::mem::size_of::<#ty>(),
                ) 
            };
            self.storage.update(record_desc, src, hasher)
        }

        pub fn #getter_name(&self, hasher: Option<&mut H>) ->  Result<Option<&'static #ty>, Error<M::Error>> {
            let record_desc = &self.record_table[#uid];
            let some = self.storage.get(record_desc, hasher)?;
            
            match some {
                Some(payload) => {
                    unsafe {
                        let field_ptr = payload.as_ptr() as usize as *const #ty;
                        Ok(Some(&*field_ptr))
                    }
                }
                None => Ok(None),
            }
        }
    )
}

fn setter_getter_static_byte_slice(name: &Ident, uid: &LitInt) -> TokenStream {
    let setter_name = Ident::new(&("set_".to_string() + &name.to_string()), Span::call_site());
    let getter_name = Ident::new(&("get_".to_string() + &name.to_string()), Span::call_site());
    quote!(
        pub fn #setter_name(&mut self, #name: &[u8], hasher: &mut H)
            -> Result<(),Error<M::Error>>
        {
            let mut record_desc = &mut self.record_table[#uid];
            self.storage.update(record_desc, #name, hasher)
        }

        pub fn #getter_name(&self, hasher: Option<&mut H>) ->  Result<Option<&'static [u8]>, Error<M::Error>> {
            let record_desc = &self.record_table[#uid];
            let some = self.storage.get(record_desc, hasher)?;
            
            match some {
                Some(payload) => Ok(Some(payload)),
                None => Ok(None),
            }
        }
    )
}

fn setter_getter_static_str(name: &Ident, uid: &LitInt) -> TokenStream {
    let setter_name = Ident::new(&("set_".to_string() + &name.to_string()), Span::call_site());
    let getter_name = Ident::new(&("get_".to_string() + &name.to_string()), Span::call_site());
    quote!(
        pub fn #setter_name(&mut self, #name: &str, hasher: &mut H)
            -> Result<(),Error<M::Error>>
        {
            let mut record_desc = &mut self.record_table[#uid];
            self.storage.update(record_desc, #name.as_bytes(), hasher)
        }

        pub fn #getter_name(&self, hasher: Option<&mut H>) ->  Result<Option<&'static str>, Error<M::Error>> {
            let record_desc = &self.record_table[#uid];
            let some = self.storage.get(record_desc, hasher)?;
            
            match some {
                Some(payload) => {
                    let str = unsafe { ::core::str::from_utf8_unchecked(payload) };
                    Ok(Some(str))
                }
                None => Ok(None),
            }
        }
    )
}











