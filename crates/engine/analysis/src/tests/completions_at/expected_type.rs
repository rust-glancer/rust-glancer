use expect_test::expect;

use crate::tests::utils::check_analysis;

#[test]
fn ranks_values_by_active_call_parameter() {
    check_analysis(
        r#"
//- /src/lib.rs crate:completion_expected_argument
struct Receiver<T>(T);
impl<T> Receiver<T> {
    fn select(&self, _: bool, _: T) {}
}

fn pair<T>(_: T, _: T) -> bool { true }
fn flag(_: bool) {}
fn explicit<T>(_: T) {}

fn demo(receiver: Receiver<u8>, choice_a_wrong: bool, choice_z_fit: u8) {
    receiver.select(#[cfg(any())] (), true, choice_$0); // complete("method uses receiver type").matching("choice_")
    flag(pair(1_u8, choice_$0)); // complete("innermost argument uses the first argument's type").matching("choice_")
    receiver.select(true, choice_a_wrong$0); // complete("receiver still supplies the type of a filled argument").matching("choice_")
    pair(1_u8, choice_a_wrong$0); // complete("sibling still supplies the type of a filled argument").matching("choice_")
    explicit::<u8>(choice_a_wrong$0); // complete("written generic argument").matching("choice_")
}

fn empty(choice_a_wrong: u8, choice_z_fit: bool) {
    flag($0); // complete("empty argument").matching("choice_")
}

fn unfinished(choice_a_wrong: u8, choice_z_fit: bool) {
    flag($0 // complete("unfinished argument list").matching("choice_")
"#,
        expect![[r#"
            // method uses receiver type
            vr choice_z_fit
            vr choice_a_wrong

            // innermost argument uses the first argument's type
            vr choice_z_fit
            vr choice_a_wrong

            // receiver still supplies the type of a filled argument
            vr choice_z_fit
            vr choice_a_wrong

            // sibling still supplies the type of a filled argument
            vr choice_z_fit
            vr choice_a_wrong

            // written generic argument
            vr choice_z_fit
            vr choice_a_wrong

            // empty argument
            vr choice_z_fit
            vr choice_a_wrong

            // unfinished argument list
            vr choice_z_fit
            vr choice_a_wrong
        "#]],
    );
}

#[test]
fn ranks_values_and_constructors_in_typed_initializers_and_assignments() {
    check_analysis(
        r#"
//- /src/lib.rs crate:completion_expected_assignment
struct ChoiceUnit;
struct ChoiceTuple(u8);
const ChoiceConst: ChoiceUnit = ChoiceUnit;
static ChoiceStatic: ChoiceUnit = ChoiceUnit;
fn ChoiceMake() -> ChoiceUnit { ChoiceUnit }
mod model {
    pub struct ChoiceOther { pub value: u8 }
    pub struct ChoiceRecord { pub value: u8 }
}

fn demo(ChoiceWrong: &ChoiceUnit, ChoiceBinding: ChoiceUnit) {
    let mut chosen: ChoiceUnit = Choice$0; // complete("annotation").matching("Choice")
    chosen = $0; // complete("assignment").matching("Choice")
    let _: ChoiceTuple = Choice$0(0); // complete("tuple constructor").matching("Choice")
    let _: model::ChoiceRecord = model::Choice$0 { value: 0 }; // complete("qualified record constructor").matching("Choice")
}
"#,
        expect![[r#"
            // annotation
            vr ChoiceBinding
            ct ChoiceConst
            fn ChoiceMake
            sc ChoiceStatic
            st ChoiceUnit
            vr ChoiceWrong
            st ChoiceTuple

            // assignment
            vr ChoiceBinding
            ct ChoiceConst
            fn ChoiceMake
            sc ChoiceStatic
            st ChoiceUnit
            vr ChoiceWrong
            st ChoiceTuple

            // tuple constructor
            st ChoiceTuple
            vr ChoiceBinding
            vr ChoiceWrong
            ct ChoiceConst
            fn ChoiceMake
            sc ChoiceStatic
            st ChoiceUnit

            // qualified record constructor
            st ChoiceRecord
            st ChoiceOther
        "#]],
    );
}

#[test]
fn ranks_values_and_enum_variants_in_return_expressions() {
    check_analysis(
        r#"
//- /src/lib.rs crate:completion_expected_return
enum Event<T> { ChoiceVariant(T), ChoiceZUnit }
impl Event<u8> {
    fn ChoiceOther() -> u16 { 0 }
}

async fn tail(choice_a_wrong: Event<u16>, choice_z_fit: Event<u8>) -> Event<u8> {
    if true {
        choice_$0 // complete("async function tail branch").matching("choice_")
    } else {
        choice_z_fit
    }
}

fn early(choice_a_wrong: Event<u16>, choice_z_fit: Event<u8>) -> Event<u8> {
    if true {
        return (choice_$0); // complete("explicit nested return").matching("choice_")
    }
    Event::<u8>::Choice$0(0) // complete("qualified constructor").matching("Choice")
}

fn unit() -> Event<u8> {
    Event::Choice$0 // complete("unit constructor before tuple constructor").matching("Choice")
}
"#,
        expect![[r#"
            // async function tail branch
            vr choice_z_fit
            vr choice_a_wrong

            // explicit nested return
            vr choice_z_fit
            vr choice_a_wrong

            // qualified constructor
            ev ChoiceVariant
            fn ChoiceOther
            ev ChoiceZUnit

            // unit constructor before tuple constructor
            ev ChoiceZUnit
            fn ChoiceOther (maybe)
            ev ChoiceVariant
        "#]],
    );
}

#[test]
fn ranks_calls_with_inserted_or_written_arguments_and_skips_unit() {
    check_analysis(
        r#"
//- /src/lib.rs crate:completion_rendered_call
fn choice_a_numeric() -> u8 { 0 }
fn choice_b_unit() {}
fn choice_z_bool<T>() -> bool { false }

fn snippets() -> bool {
    choice_$0 // complete("inserts a call").matching("choice_")
}

fn written_arguments() -> bool {
    choice_$0::<u8>() // complete_verbose("keeps the written arguments").matching("choice_z_bool")
}

fn unit() {
    choice_$0 // complete("unit leaves scope order intact").matching("choice_")
}
"#,
        expect![[r#"
            // inserts a call
            fn choice_z_bool
            fn choice_a_numeric
            fn choice_b_unit

            // keeps the written arguments
            fn choice_z_bool
              detail: fn choice_z_bool<T>() -> bool
              sort: !expected|01-module|choice_z_bool|06|00|Function(FunctionRef { store: ItemStoreRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), id: Scope }, id: FunctionId(2) })
              replace: 173..180

            // unit leaves scope order intact
            fn choice_a_numeric
            fn choice_b_unit
            fn choice_z_bool
        "#]],
    );
}

#[test]
fn suggests_missing_variants_of_the_expected_enum() {
    check_analysis(
        r#"
//- /src/lib.rs crate:completion_expected_variants
mod model {
    pub enum Event<T> {
        ChoiceUnit,
        ChoiceTuple(T),
        ChoiceRecord { value: T },
    }
}
use model::Event as Imported;

fn aliased(ChoiceWrong: u8) -> Imported<u8> {
    Cho$0 // complete_verbose("aliased enum constructors").matching("Choice")
}

fn tuple() -> Imported<u8> {
    Cho$0(1) // complete_verbose("existing tuple arguments").matching("Choice")
}

fn record() {
    let _: Imported<u8> = Cho$0 { value: 1 }; // complete_verbose("existing record fields").matching("Choice")
}

mod nested {
    fn qualified() -> crate::model::Event<u8> {
        Cho$0 // complete_verbose("reachable enum outside scope").matching("ChoiceUnit")
    }
}

mod in_scope {
    use crate::model::Event::{self as Imported, ChoiceTuple};
    fn use_short_name() -> Imported<u8> {
        Cho$0 // complete_verbose("variant in scope uses its short name").matching("ChoiceTuple")
    }
}

fn shadowed() -> Imported<u8> {
    struct Imported;
    Cho$0 // complete_verbose("shadowed enum uses its qualified path").matching("ChoiceUnit")
}

fn generic<Imported>() -> crate::model::Event<u8> {
    Cho$0 // complete_verbose("type parameter shadows the imported enum name").matching("ChoiceUnit")
}
"#,
        expect![[r#"
            // aliased enum constructors
            variant ChoiceRecord
              detail: variant Imported::ChoiceRecord
              sort: !expected|00-expected|ChoiceRecord|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 2 })
              replace: 204..207
              snippet: Imported::ChoiceRecord { $0 }
            variant ChoiceTuple
              detail: variant Imported::ChoiceTuple
              sort: !expected|00-expected|ChoiceTuple|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 1 })
              replace: 204..207
              snippet: Imported::ChoiceTuple($0)
            variant ChoiceUnit
              detail: variant Imported::ChoiceUnit
              sort: !expected|00-expected|ChoiceUnit|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 204..207
              insert: Imported::ChoiceUnit
            variable ChoiceWrong
              detail: let ChoiceWrong: u8
              sort: 00-body:0001|ChoiceWrong|07|00|Declaration(DeclarationRef { kind: "binding", body: FunctionBodyRef { crate_ref: CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }, body: BodyId(0) }, binding: BindingId(0) })
              replace: 204..207

            // existing tuple arguments
            variant ChoiceTuple
              detail: variant Imported::ChoiceTuple
              sort: !expected|00-expected|ChoiceTuple|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 1 })
              replace: 244..247
              insert: Imported::ChoiceTuple

            // existing record fields
            variant ChoiceRecord
              detail: variant Imported::ChoiceRecord
              sort: !expected|00-expected|ChoiceRecord|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 2 })
              replace: 294..297
              insert: Imported::ChoiceRecord

            // reachable enum outside scope
            variant ChoiceUnit
              detail: variant crate::model::Event::ChoiceUnit
              sort: !expected|00-expected|ChoiceUnit|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 384..387
              insert: crate::model::Event::ChoiceUnit

            // variant in scope uses its short name
            variant ChoiceTuple
              detail: variant ChoiceTuple
              sort: !expected|01-module|ChoiceTuple|04|00|Declaration(DeclarationRef { kind: "enum_variant", variant: EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 1 } })
              replace: 524..527
              snippet: ChoiceTuple($0)

            // shadowed enum uses its qualified path
            variant ChoiceUnit
              detail: variant crate::model::Event::ChoiceUnit
              sort: !expected|00-expected|ChoiceUnit|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 594..597
              insert: crate::model::Event::ChoiceUnit

            // type parameter shadows the imported enum name
            variant ChoiceUnit
              detail: variant crate::model::Event::ChoiceUnit
              sort: !expected|00-expected|ChoiceUnit|04|00|EnumVariant(EnumVariantRef { origin: Crate(CrateRef { package: PackageSlot(0), crate_id: CrateId(0) }), enum_id: EnumId(0), index: 0 })
              replace: 657..660
              insert: crate::model::Event::ChoiceUnit
        "#]],
    );
}
