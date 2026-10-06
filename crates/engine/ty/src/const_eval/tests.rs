use super::ConstValue::{self, Scalar, Unknown};

#[test]
fn evaluates_const_expressions() {
    let cases = [
        ("2", Scalar(2), "decimal literal"),
        ("1_024usize", Scalar(1024), "separator and usize suffix"),
        ("0x10 + 0o10 + 0b10", Scalar(26), "integer bases"),
        ("2 + 2", Scalar(4), "addition"),
        ("10 - 3 - 2", Scalar(5), "left associative subtraction"),
        ("2 + 3 * 4", Scalar(14), "operator precedence"),
        ("(2 + 3) * 4", Scalar(20), "parentheses"),
        ("(20 / 3) % 4", Scalar(2), "division and remainder"),
        ("{ 2 + { 3 * 4 } }", Scalar(14), "expression-only blocks"),
        (" 2 /* operand */ + 2 ", Scalar(4), "trivia"),
        ("4_294_967_294 + 1", Scalar(4_294_967_295), "32-bit maximum"),
        ("1 << 4", Scalar(16), "left shift"),
        ("0x80 >> 3", Scalar(16), "right shift"),
        ("1 << 2 + 2", Scalar(16), "arithmetic before shift"),
        ("1 << 2 << 1", Scalar(8), "left associative shifts"),
        ("0b1010 & 0b1100", Scalar(8), "bitwise and"),
        ("0b1010 | 0b1100", Scalar(14), "bitwise or"),
        ("0b1010 ^ 0b1100", Scalar(6), "bitwise xor"),
        ("!0 & 0xff", Scalar(255), "bitwise not"),
        ("0xff & !(1 << 3)", Scalar(247), "clearing a bit"),
        ("8 | 6 ^ 3 & 5", Scalar(15), "bitwise precedence"),
        ("1 << (2 + 2u8)", Scalar(16), "unsigned shift count"),
        ("1 << (32i16 >> 3)", Scalar(16), "signed shift count"),
        (
            "1 << ((0xf0u8 << 1) >> 6)",
            Scalar(8),
            "nested shifts use the left operand's width",
        ),
        (
            "1 << (!0u8 & 7)",
            Scalar(128),
            "bitwise not in an unsigned shift count",
        ),
        (
            "1 << (1u8 + 1u16)",
            Unknown,
            "conflicting shift count types",
        ),
        ("SIZE", Unknown, "named const"),
        ("2 + SIZE", Unknown, "named operand after a literal"),
        ("2 + Foo::BAR", Unknown, "associated const"),
        ("2 + size()", Unknown, "const function call"),
        ("2 + (4u8 as usize)", Unknown, "cast"),
        (
            "{ let size = 2; size + 2 }",
            Unknown,
            "block with statements",
        ),
        ("2.0", Unknown, "float literal"),
        ("2f32", Unknown, "float suffix on an integer token"),
        ("2u8 + 2u8", Unknown, "u8 arithmetic"),
        ("4_294_967_295 + 1 - 1", Unknown, "intermediate overflow"),
        ("0 - 1 + 1", Unknown, "intermediate underflow"),
        ("2 / 0", Unknown, "division by zero"),
        ("2 % 0", Unknown, "remainder by zero"),
        ("2 +", Unknown, "incomplete expression"),
        ("2 3", Unknown, "trailing tokens"),
    ];
    for (text, expected, message) in cases {
        assert_eq!(
            ConstValue::from_syntax(text, Some(32)),
            expected,
            "{message}"
        );
    }
}

#[test]
fn uses_compilation_target_width() {
    let cases = [
        ("4_294_967_295 + 1", Some(32), Unknown, "32-bit addition"),
        (
            "4_294_967_295 + 1",
            Some(64),
            Scalar(4_294_967_296),
            "64-bit addition",
        ),
        (
            "4_294_967_295 + 1",
            None,
            Unknown,
            "missing compilation target",
        ),
        ("1 << 32", Some(32), Unknown, "left shift by the type width"),
        (
            "1 >> 32",
            Some(32),
            Unknown,
            "right shift by the type width",
        ),
        (
            "1 << 32",
            Some(64),
            Scalar(4_294_967_296),
            "64-bit left shift",
        ),
        ("1 >> 32", Some(64), Scalar(0), "64-bit right shift"),
        (
            "0x8000_0000 << 1",
            Some(32),
            Scalar(0),
            "left shift discards the top bit",
        ),
        (
            "0x8000_0000 << 1",
            Some(64),
            Scalar(4_294_967_296),
            "left shift retains bits inside the type width",
        ),
        (
            "!0usize",
            Some(32),
            Scalar(u128::from(u32::MAX)),
            "32-bit not",
        ),
        (
            "!0usize",
            Some(64),
            Scalar(u128::from(u64::MAX)),
            "64-bit not",
        ),
        ("!0usize", Some(128), Scalar(u128::MAX), "128-bit not"),
        ("!0 >> 31", Some(32), Scalar(1), "right shift after not"),
        (
            "!0 << 1 >> 1",
            Some(32),
            Scalar(2_147_483_647),
            "discarded bits stay discarded",
        ),
        (
            "1 << (0x7fff_ffff + 1 - 0x7fff_ffff)",
            Some(64),
            Unknown,
            "unsuffixed shift count arithmetic uses i32",
        ),
        (
            "1 << (!0 >> 31)",
            Some(32),
            Unknown,
            "unsuffixed shift count is signed",
        ),
    ];
    for (text, pointer_width, expected, message) in cases {
        assert_eq!(
            ConstValue::from_syntax(text, pointer_width),
            expected,
            "{message}"
        );
    }
}
