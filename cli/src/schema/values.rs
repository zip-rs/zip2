use super::backends::Backend;

/* pub trait SchemaValue<B: Backend>: Sized { */
/*     type DeserErr; */
/*     fn serialize(self) -> <B as Backend>::Value; */
/*     fn deserialize(s: <B as Backend>::Value) -> Result<Self, Self::DeserErr>; */
/* } */

pub trait NamedList {
    fn f(self);
}

/* pub enum Schema { */
/*     Bool, */
/*     Str, */
/*     Arr, */
/*     Obj, */
/*     Arr(Vec<Box<Schema>>) */
/*     Str(String), */
/*     Arr(Vec<Box<Schema>>), */
/*     Obj(Vec<(String, Box<Schema>)>), */
/* } */

/* pub trait Schema {} */

/* pub enum Command { */
/*     /// Write a JSON object to stdout which contains all the file paths under */
/*     /// the top-level `paths`. */
/*     Crawl { */
/*         #[command(flatten)] */
/*         crawl: MedusaCrawl, */
/*     }, */
/*     /// Consume a JSON object from [`Self::Crawl`] over stdin and write those */
/*     /// files into a zip file at `output`. */
/*     Zip { */
/*         #[command(flatten)] */
/*         output: Output, */
/*         #[command(flatten)] */
/*         zip_options: ZipOutputOptions, */
/*         #[command(flatten)] */
/*         modifications: EntryModifications, */
/*         #[arg(long, value_enum, default_value_t)] */
/*         parallelism: Parallelism, */
/*     }, */
/*     /// Merge the content of several zip files into one. */
/*     Merge { */
/*         #[command(flatten)] */
/*         output: Output, */
/*         /// ??? */
/*         #[command(flatten)] */
/*         mtime_behavior: ModifiedTimeBehavior, */
/*         #[command(flatten)] */
/*         merge: MedusaMerge, */
/*     }, */
/*     /// Perform a `crawl` and then a `zip` on its output in memory. */
/*     CrawlZip { */
/*         #[command(flatten)] */
/*         crawl: MedusaCrawl, */
/*         #[command(flatten)] */
/*         output: Output, */
/*         #[command(flatten)] */
/*         zip_options: ZipOutputOptions, */
/*         #[command(flatten)] */
/*         modifications: EntryModifications, */
/*         #[arg(long, value_enum, default_value_t)] */
/*         parallelism: Parallelism, */
/*     }, */
/*     /// Perform a `zip` and then a `merge` without releasing the output file */
/*     /// handle. */
/*     ZipMerge { */
/*         #[command(flatten)] */
/*         output: Output, */
/*         #[command(flatten)] */
/*         zip_options: ZipOutputOptions, */
/*         #[command(flatten)] */
/*         modifications: EntryModifications, */
/*         #[arg(long, value_enum, default_value_t)] */
/*         parallelism: Parallelism, */
/*         #[command(flatten)] */
/*         merge: MedusaMerge, */
/*     }, */
/*     /// Perform `crawl`, then a `zip` on its output in memory, then a `merge` */
/*     /// into the same output file. */
/*     CrawlZipMerge { */
/*         #[command(flatten)] */
/*         crawl: MedusaCrawl, */
/*         #[command(flatten)] */
/*         output: Output, */
/*         #[command(flatten)] */
/*         zip_options: ZipOutputOptions, */
/*         #[command(flatten)] */
/*         modifications: EntryModifications, */
/*         #[arg(long, value_enum, default_value_t)] */
/*         parallelism: Parallelism, */
/*         #[command(flatten)] */
/*         merge: MedusaMerge, */
/*     }, */
/* } */

pub enum HydratedValue<'a> {
    Bool(bool),
    Str(&'a str),
    Arr(Vec<Box<HydratedValue<'a>>>),
    Obj(Vec<(&'a str, Box<HydratedValue<'a>>)>),
}

pub trait Hydrate<Value> {
    fn hydrate(v: HydratedValue) -> Value;
}

pub trait Schema: Backend {
    fn print<'a>(v: HydratedValue<'a>) -> <Self as Backend>::Val<'a>;
}

#[cfg(feature = "json")]
pub mod json_value {
    use super::*;
    use crate::schema::backends::json_backend::JsonBackend;

    /* impl SchemaValue<JsonBackend> for bool { */
    /*     type DeserErr = String; */

    /*     fn serialize(self) -> json::JsonValue { */
    /*         json::JsonValue::Boolean(self) */
    /*     } */
    /*     fn deserialize(s: json::JsonValue) -> Result<Self, String> { */
    /*         match s { */
    /*             json::JsonValue::Boolean(value) => Ok(value), */
    /*             s => Err(format!("non-boolean value {s}")), */
    /*         } */
    /*     } */
    /* } */

    /* impl SchemaValue<JsonBackend> for String { */
    /*     type DeserErr = String; */

    /*     fn serialize(self) -> json::JsonValue { */
    /*         json::JsonValue::String(self) */
    /*     } */
    /*     fn deserialize(s: json::JsonValue) -> Result<Self, String> { */
    /*         match s { */
    /*             json::JsonValue::String(value) => Ok(value), */
    /*             s => Err(format!("non-string value {s}")), */
    /*         } */
    /*     } */
    /* } */
}

/* pub enum SchemaValue { */
/*     Bool(bool), */
/*     Path(PathBuf), */
/* } */

/* pub trait SchemaValue<B: Backend> {} */

/* impl SchemaValue for bool {} */
