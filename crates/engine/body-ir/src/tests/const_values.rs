use rg_ir_model::{EvaluatedConstValue, PackageSlot, UnsignedIntTy};

use crate::testonly::BodyIrFixture;

#[test]
fn evaluates_initializers_with_semantic_types_independently_of_body_policy() {
    let fixture = BodyIrFixture::build(
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
    let stores = (0..fixture.semantic_ir_db().package_count())
        .filter_map(|index| {
            fixture
                .semantic_ir_db()
                .resident_package(PackageSlot(index))
        })
        .flat_map(|package| package.crates())
        .map(|krate| krate.items())
        .collect::<Vec<_>>();
    for (name, value) in [
        ("DEPENDENCY", 16),
        ("CAPACITY", 16),
        ("DEFAULT", 16),
        ("PROJECTED", 16),
        ("ZERO", 0),
    ] {
        let konst = stores
            .iter()
            .flat_map(|items| items.consts().iter())
            .find(|data| data.name.as_str() == name)
            .expect("fixture should contain the const declaration");
        assert_eq!(
            konst.value.as_deref(),
            Some(&EvaluatedConstValue::UnsignedInt {
                value,
                ty: UnsignedIntTy::Usize
            }),
            "{name} uses shared type lowering and evaluation",
        );
    }
    for (name, value) in [
        ("DEPENDENCY_STATIC", 437),
        ("STATIC_CAPACITY", 1024),
        ("COUNTER", 4),
        ("STATIC_PROJECTED", 16),
    ] {
        let static_ = stores
            .iter()
            .flat_map(|items| items.statics().iter())
            .find(|data| data.name.as_str() == name)
            .expect("fixture should contain the static declaration");
        assert_eq!(
            static_.value.as_deref(),
            Some(&EvaluatedConstValue::UnsignedInt {
                value,
                ty: UnsignedIntTy::Usize
            }),
            "{name} uses shared type lowering and evaluation",
        );
    }
}
