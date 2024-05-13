use std::env::var;

fn main() {
    if var("CARGO_FEATURE_DEFLATE_MINIZ").is_ok() {
        println!("cargo:warning=Feature `deflate-miniz` is deprecated; replace it with `deflate`");
    }
    #[cfg(not(any(feature = "sync", feature = "tokio")))]
    compile_error!("Missing Required feature");

    #[cfg(all(feature = "sync", feature = "tokio"))]
    compile_error!("The features sync and tokio cannot be used together")
}
