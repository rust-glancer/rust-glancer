use rg_ir_model::{EvaluatedConstValue, UnsignedIntTy};
use rg_workspace::TargetKind;

use crate::{saved_body, testonly::BodyIrFixture};

#[test]
fn evaluates_const_and_static_initializers_with_semantic_types() {
    let mut fixture = BodyIrFixture::build(
        r#"
//- /sizes/src/lib.rs crate:sizes root:sizes origin:dependency
pub type Size = usize;
pub const DEPENDENCY: Size = (2 + 2) * 4;
pub static DEPENDENCY_STATIC: Size = 7 * 60 + 17;

//- /app/src/lib.rs crate:app root:app deps:sizes
use sizes::Size;
type Identity<T> = T;
type Count = Identity<Size>;
pub const CAPACITY: Count = (2 + 2) * 4;
pub static STATIC_CAPACITY: Count = 1 << 10;
pub static mut COUNTER: Count = 2 + 2;

pub struct Buffer;
impl Buffer {
    pub const DEFAULT: Size = 1 << (2 + 2u8);
}

trait Length { type Size; }
impl Length for Buffer { type Size = usize; }
pub const PROJECTED: <Buffer as Length>::Size = 16;
pub static STATIC_PROJECTED: <Buffer as Length>::Size = 2 * 8;

impl usize {
    pub const ZERO: Self = 0;
}
"#,
    );
    fixture.inspect(|view| {
        let semantic = view.semantic();
        let app = semantic.crate_ref("app", TargetKind::Lib);
        let dependency = semantic.crate_ref("sizes", TargetKind::Lib);
        for (target, name, value) in [
            (dependency, "DEPENDENCY", 16),
            (dependency, "DEPENDENCY_STATIC", 437),
            (app, "CAPACITY", 16),
            (app, "DEFAULT", 16),
            (app, "PROJECTED", 16),
            (app, "ZERO", 0),
            (app, "STATIC_CAPACITY", 1024),
            (app, "COUNTER", 4),
            (app, "STATIC_PROJECTED", 16),
        ] {
            let key = view.body_key(target, name);
            let saved = saved_body(semantic.db(), key).expect("fixture initializer query");
            let body = saved.as_ref().expect("fixture initializer should exist");
            assert_eq!(
                body.prepared.initializer.as_ref(),
                Some(&EvaluatedConstValue::UnsignedInt {
                    value,
                    ty: UnsignedIntTy::Usize,
                }),
                "{name} uses shared type lowering and evaluation",
            );
        }
    });
}
