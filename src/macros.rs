//! Macros used internally.

/// Similar to [`cfg_if`](cfg_if), but accepts a list of expressions, and generates an internal
/// closure to return each value.
///
/// The main reason this is necessary is because attaching `#[cfg(...)]` annotations to certain
/// types of statements requires a nightly feature, or `cfg_if` would be enough on its own. This
/// macro's restricted interface allows it to generate a closure as a circumlocution that is legal
/// on stable rust.
///
/// Note that any `return` operation within the expressions provided to this macro will apply to the
/// generated closure, not the enclosing scope--it cannot be used to interfere with external
/// control flow.
///
/// The generated closure is non-[`const`](const@keyword), so cannot be used inside `const` methods.
macro_rules! cfg_if_expr {
    // Match =>, chains, maybe with a final _ => catchall clause.
    (
        $( $ret_ty:ty : )?
        $(
            #[cfg( $i_meta:meta )] => $i_val:expr
        ),+ ,
            _ => $rem_val:expr $(,)?
    ) => {
        (|| $( -> $ret_ty )? {
            $crate::cfg_if_expr! {
                @__items ();
                $(
                    (( $i_meta ) (
                        #[allow(unreachable_code)]
                        return $i_val ;
                    )) ,
                )+
                    (() (
                        #[allow(unreachable_code)]
                        return $rem_val ;
                    )) ,
            }
        })()
    };
    // Match =>, chains *without* any _ => clause.
    (
        $( $ret_ty:ty : )?
        $(
            #[cfg( $i_meta:meta )] => $i_val:expr
        ),+ $(,)?
    ) => {
        (|| $( -> $ret_ty )? {
            $crate::cfg_if_expr! {
                @__items ();
                $(
                    (( $i_meta ) (
                        #[allow(unreachable_code)]
                        return $i_val ;
                    )) ,
                )+
            }
        })()
    };

    (@__items ( $( $_:meta , )* ) ; ) => {};
    (
        @__items ( $( $no:meta , )* ) ;
        (( $( $yes:meta )? ) ( $( $tokens:tt )* )) ,
        $( $rest:tt , )*
    ) => {
        #[cfg(all(
            $( $yes , )?
            not(any( $( $no ),* ))
        ))]
        $crate::cfg_if_expr! { @__identity $( $tokens )* }

        $crate::cfg_if_expr! {
            @__items ( $( $no , )* $( $yes , )? ) ;
            $( $rest , )*
        };
    };
    (@__identity $( $tokens:tt )* ) => {
        $( $tokens )*
    };
}
pub(crate) use cfg_if_expr;
