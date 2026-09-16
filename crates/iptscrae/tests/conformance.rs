//! Conformance suite: one case per documented operator, command and
//! control-flow form.
//!
//! Every expectation is traceable to the *Iptscrae Language Guide* or to
//! OpenPalace's ActionScript VM. Where the two disagree the case says so and the
//! README records the resolution. This suite is the contract the corpus run is
//! checked against: if it passes and the corpus runs clean, the core behaves.
//!
//! A note on trailing `-`: the reference tokenizer treats a `-` with nothing
//! after it as an integer literal (`parseInt("-")` is `NaN`, which becomes 0),
//! so a subtraction at the very end of a source string needs a trailing space.
//! Several cases below show that explicitly.

use iptscrae::testing::{eval, eval_int, eval_text, TestHost};
use iptscrae::{Engine, Value};

fn int(source: &str) -> i32 {
    eval_int(source).unwrap_or_else(|e| panic!("{source:?} -> {e}"))
}

fn text(source: &str) -> String {
    eval_text(source).unwrap_or_else(|e| panic!("{source:?} -> {e}"))
}

fn fails(source: &str) -> iptscrae::IptError {
    eval(source).expect_err(&format!("{source:?} should have failed"))
}

// ---------------------------------------------------------------- literals

#[test]
fn integer_literals_are_decimal_and_signed() {
    assert_eq!(int("0"), 0);
    assert_eq!(int("42"), 42);
    assert_eq!(int("-42"), -42);
    assert_eq!(int("012"), 12, "leading zeros are decimal, not octal");
    assert_eq!(int("2147483647"), 2147483647);
}

#[test]
fn there_are_no_floats() {
    assert_eq!(fails("1.5").category(), "lex");
}

#[test]
fn a_dash_at_end_of_input_is_a_zero_literal() {
    assert_eq!(int("5 -"), 0, "parseInt(\"-\") is NaN, which coerces to 0");
    assert_eq!(int("5 3 - "), 2, "with a following space it is subtraction");
}

#[test]
fn strings_preserve_case_and_only_recognise_the_hex_escape() {
    assert_eq!(text("\"Hello, World!\""), "Hello, World!");
    assert_eq!(text("\"\""), "");
    assert_eq!(text("\"a\\\"b\""), "a\"b");
    assert_eq!(
        text("\"tab\\there\""),
        "tabthere",
        "\\t is not a tab escape"
    );
    assert_eq!(text("\"\\x41\\x42\""), "AB");
    assert_eq!(text("\"one\ntwo\""), "one\ntwo", "strings may span lines");
}

#[test]
fn comments_run_to_end_of_line_in_both_spellings() {
    assert_eq!(int("1 # ignored\n2 +"), 3);
    assert_eq!(int("1 ; ignored\n2 +"), 3);
    assert_eq!(int("{ 1 # } is not a brace\n 2 + } EXEC"), 3);
}

// ------------------------------------------------------------------- stack

#[test]
fn stack_commands() {
    assert_eq!(int("1 DUP +"), 2);
    assert_eq!(int("1 2 SWAP - "), 1);
    assert_eq!(int("1 2 OVER - - "), 0);
    assert_eq!(int("10 20 30 0 PICK"), 30);
    assert_eq!(int("10 20 30 2 PICK"), 10);
    assert_eq!(int("1 2 3 STACKDEPTH"), 3);
    assert_eq!(int("1 POP 2"), 2);
    assert_eq!(int("TOPTYPE"), 0);
}

#[test]
fn stack_underflow_is_an_error() {
    assert_eq!(fails("POP").category(), "stack");
    assert_eq!(fails("1 SWAP").category(), "stack");
    assert_eq!(fails("OVER").category(), "stack");
}

#[test]
fn toptype_codes_match_the_guide() {
    assert_eq!(int("7 TOPTYPE"), 1);
    assert_eq!(int("7 v = v TOPTYPE"), 2);
    assert_eq!(int("{ } TOPTYPE"), 3);
    assert_eq!(int("\"s\" TOPTYPE"), 4);
    assert_eq!(int("[ TOPTYPE"), 5);
    assert_eq!(int("[] TOPTYPE"), 6);
    assert_eq!(int("7 v = v VARTYPE"), 1, "VARTYPE dereferences");
}

// ---------------------------------------------------------------- arithmetic

#[test]
fn the_five_arithmetic_operators() {
    assert_eq!(int("2 3 +"), 5);
    assert_eq!(int("3 2 - "), 1);
    assert_eq!(int("2 3 *"), 6);
    assert_eq!(int("7 2 /"), 3);
    assert_eq!(int("7 2 %"), 1);
}

#[test]
fn division_truncates_toward_zero() {
    assert_eq!(int("-7 2 /"), -3);
    assert_eq!(int("7 -2 /"), -3);
    assert_eq!(int("-7 -2 /"), 3);
}

#[test]
fn division_and_modulo_by_zero_yield_zero() {
    assert_eq!(int("1 0 /"), 0);
    assert_eq!(int("0 0 /"), 0);
    assert_eq!(int("1 0 %"), 0);
}

#[test]
fn modulo_sign_follows_the_dividend() {
    assert_eq!(int("-7 3 %"), -1);
    assert_eq!(int("7 -3 %"), 1);
}

#[test]
fn integers_wrap_at_32_bits() {
    assert_eq!(int("2147483647 1 +"), i32::MIN);
    assert_eq!(int("-2147483648 1 - "), i32::MAX);
}

#[test]
fn concatenation() {
    assert_eq!(text("\"a\" \"b\" &"), "ab");
    assert_eq!(text("\"a\" \"b\" +"), "ab", "+ concatenates two strings");
    assert_eq!(fails("1 \"b\" &").category(), "type");
    assert_eq!(fails("1 \"b\" +").category(), "type");
}

#[test]
fn assignment_operators() {
    assert_eq!(int("5 x = x"), 5);
    assert_eq!(int("5 x = 3 x += x"), 8);
    assert_eq!(int("5 x = 3 x -= x"), 2);
    assert_eq!(int("5 x = 3 x *= x"), 15);
    assert_eq!(int("12 x = 3 x /= x"), 4);
    assert_eq!(int("12 x = 5 x %= x"), 2);
    assert_eq!(int("5 x = x ++ x"), 6);
    assert_eq!(int("5 x = x -- x"), 4);
    assert_eq!(text("\"a\" x = \"b\" x += x"), "ab");
    assert_eq!(text("\"a\" x = \"b\" x &= x"), "ab");
}

#[test]
fn compound_assignment_targets_a_variable_not_a_value() {
    assert_eq!(fails("3 1 +=").category(), "type");
}

#[test]
fn def_stores_whatever_is_under_the_symbol() {
    assert_eq!(int("{ 1 } f DEF f EXEC"), 1);
    assert_eq!(int("5 f DEF f"), 5);
}

// ------------------------------------------------------------------ strings

#[test]
fn string_commands() {
    assert_eq!(text("1 2 + ITOA"), "3");
    assert_eq!(int("\"3\" ATOI"), 3);
    assert_eq!(int("\"3x\" ATOI"), 3);
    assert_eq!(int("\"x\" ATOI"), 0);
    assert_eq!(int("\"hello\" STRLEN"), 5);
    assert_eq!(int("\"hello\" \"ell\" STRINDEX"), 1);
    assert_eq!(int("\"hello\" \"z\" STRINDEX"), -1);
    assert_eq!(text("\"hello\" 1 3 SUBSTRING"), "ell");
    assert_eq!(
        int("\"hello\" \"ELL\" SUBSTR"),
        1,
        "SUBSTR is case-insensitive"
    );
    assert_eq!(text("\"HeLLo\" LOWERCASE"), "hello");
    assert_eq!(text("\"HeLLo\" UPPERCASE"), "HELLO");
    assert_eq!(int("\"1 2 +\" STRTOATOM EXEC"), 3);
}

#[test]
fn substring_rejects_a_negative_offset() {
    assert_eq!(fails("\"abc\" -1 2 SUBSTRING").category(), "type");
}

// -------------------------------------------------------------- comparisons

#[test]
fn equality_is_case_insensitive_for_strings() {
    assert_eq!(int("\"abc\" \"ABC\" =="), 1);
    assert_eq!(int("\"abc\" \"abd\" =="), 0);
    assert_eq!(int("2 2 =="), 1);
    assert_eq!(int("\"2\" 2 =="), 0, "mixed types are never equal");
}

#[test]
fn inequality_is_case_sensitive_for_strings() {
    assert_eq!(int("\"abc\" \"ABC\" !="), 1, "the reference's asymmetry");
    assert_eq!(int("\"abc\" \"ABC\" <>"), 1);
    assert_eq!(int("\"abc\" \"abc\" !="), 0);
    assert_eq!(int("\"2\" 2 !="), 1, "mixed types are not equal");
}

#[test]
fn ordering_operators() {
    assert_eq!(int("1 2 <"), 1);
    assert_eq!(int("2 2 <="), 1);
    assert_eq!(int("3 2 >"), 1);
    assert_eq!(int("2 2 >="), 1);
    assert_eq!(int("\"abc\" \"ABD\" <"), 1, "strings compare upper-cased");
}

#[test]
fn ordering_a_string_against_a_number_is_an_error() {
    assert_eq!(fails("\"a\" 1 <").category(), "type");
}

#[test]
fn logical_operators_use_truthiness() {
    assert_eq!(int("1 1 AND"), 1);
    assert_eq!(int("1 0 AND"), 0);
    assert_eq!(int("0 1 OR"), 1);
    assert_eq!(int("0 0 OR"), 0);
    assert_eq!(int("1 NOT"), 0);
    assert_eq!(int("0 NOT"), 1);
    assert_eq!(int("1 !"), 0, "! is a synonym");
    assert_eq!(int("\"\" NOT"), 0, "an empty string is true");
}

// ------------------------------------------------------------- control flow

#[test]
fn if_runs_the_body_when_the_condition_is_non_zero() {
    assert_eq!(int("1 { 7 } 1 IF"), 7);
    assert_eq!(int("1 { 7 } 0 IF"), 1);
    assert_eq!(int("1 { 7 } 2 IF"), 7, "any non-zero is true");
}

#[test]
fn ifelse_takes_the_true_clause_first() {
    assert_eq!(int("{ 1 } { 2 } 1 IFELSE"), 1);
    assert_eq!(int("{ 1 } { 2 } 0 IFELSE"), 2);
}

#[test]
fn while_takes_the_body_first_then_the_condition() {
    assert_eq!(int("0 x = { x ++ } { x 3 < } WHILE x"), 3);
    assert_eq!(int("0 x = { x ++ } { 0 } WHILE x"), 0, "false up front");
}

#[test]
fn break_leaves_a_loop() {
    assert_eq!(int("0 x = { x ++ { BREAK } x 3 == IF } { 1 } WHILE x"), 3);
}

#[test]
fn return_leaves_an_atomlist_and_exit_leaves_the_script() {
    assert_eq!(int("{ 1 RETURN 2 } EXEC"), 1);
    assert_eq!(int("1 EXIT 2"), 1);
}

#[test]
fn exec_runs_an_atomlist_and_zero_is_a_silent_no_op() {
    assert_eq!(int("1 { 2 + } EXEC"), 3);
    assert_eq!(int("1 0 EXEC"), 1);
    assert_eq!(fails("1 \"x\" EXEC").category(), "type");
}

#[test]
fn foreach_pushes_the_elements_in_order() {
    assert_eq!(int("0 s = { s += } [ 1 2 3 4 ] FOREACH s"), 10);
    assert_eq!(int("0 n = { n ++ } [] FOREACH n"), 0);
    assert_eq!(int("{ POP } [ 7 8 9 ] FOREACH STACKDEPTH"), 0);
}

// ---------------------------------------------------------------- variables

#[test]
fn variables_auto_vivify_to_zero_and_are_case_insensitive() {
    assert_eq!(int("fresh"), 0);
    assert_eq!(int("5 x = X"), 5);
    assert_eq!(int("5 mixed = MIXED"), 5);
}

#[test]
fn global_makes_a_variable_visible_to_later_activations() {
    let mut engine = Engine::new(TestHost::seeded(1));
    engine.run_source("7 g GLOBAL g =").unwrap();
    assert_eq!(
        engine.run_source_resolved("g GLOBAL g").unwrap(),
        vec![Value::Int(7)]
    );
}

#[test]
fn locals_do_not_survive_an_activation() {
    let mut engine = Engine::new(TestHost::seeded(1));
    engine.run_source("7 local =").unwrap();
    assert_eq!(
        engine.run_source_resolved("local").unwrap(),
        vec![Value::Int(0)]
    );
}

#[test]
fn global_copies_an_existing_local_value_then_aliases_it() {
    assert_eq!(int("9 v = v GLOBAL v"), 9);
    assert_eq!(int("v GLOBAL 4 v = v"), 4);
}

// ------------------------------------------------------------------- arrays

#[test]
fn arrays_are_built_by_brackets_and_can_be_read_and_written() {
    assert_eq!(int("[ 10 20 30 ] 0 GET"), 10);
    assert_eq!(int("[ 10 20 30 ] 2 GET"), 30);
    assert_eq!(int("[ 10 20 30 ] LENGTH"), 3);
    assert_eq!(int("[ 1 2 3 ] a = 9 a 1 PUT a 1 GET"), 9);
    assert_eq!(int("3 ARRAY LENGTH"), 3);
    assert_eq!(int("3 ARRAY 1 GET"), 0);
}

#[test]
fn array_contents_are_evaluated() {
    assert_eq!(int("[ 1 2 + ] 0 GET"), 3);
    assert_eq!(int("[ { 1 } { 2 } 0 IFELSE ] 0 GET"), 2);
}

#[test]
fn negative_array_size_pushes_zero() {
    assert_eq!(int("-1 ARRAY TOPTYPE"), 1, "not an array");
    assert_eq!(int("-1 ARRAY"), 0);
}

#[test]
fn array_index_errors() {
    assert_eq!(fails("[ 1 2 ] 2 GET").category(), "type");
    assert_eq!(fails("[ 1 2 ] -1 GET").category(), "type");
    assert_eq!(fails("[ 1 2 ] 5 0 PUT").category(), "type");
}

#[test]
fn arrays_are_reference_values() {
    assert_eq!(int("[ 1 ] a = 5 a 0 PUT a 0 GET"), 5);
}

// ------------------------------------------------------------------- regex

#[test]
fn grepstr_drives_a_branch_and_grepsub_substitutes() {
    assert_eq!(int("\"2 plus 3\" \"^(.*) plus (.*)$\" GREPSTR"), 1);
    assert_eq!(int("\"hello\" \"^z\" GREPSTR"), 0);
    assert_eq!(
        text("\"a1b\" \"[0-9]+\" GREPSTR POP \"<$0>\" GREPSUB"),
        "<1>"
    );
}
