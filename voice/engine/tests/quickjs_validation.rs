#![cfg(feature = "quickjs")]

use rquickjs::{Context, Module, Runtime};

pub fn validate_javascript(script: &str) -> Vec<String> {
    let rt = Runtime::new().unwrap();
    let ctx = Context::full(&rt).unwrap();
    let result = ctx.with(|ctx: rquickjs::Ctx| {
        let wrapped = format!("function __validate__() {{\n{}\n}}", script);
        match Module::declare(ctx, "validate", wrapped) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("JS Syntax Error: {}", e)),
        }
    });
    match result {
        Ok(()) => vec![],
        Err(msg) => vec![msg],
    }
}

#[test]
fn test_validate_javascript_valid() {
    let script = r#"
        let a = 1;
        let b = 2;
        return a + b;
    "#;
    let errors = validate_javascript(script);
    assert!(
        errors.is_empty(),
        "Valid script should not produce errors, got: {:?}",
        errors
    );
}

#[test]
fn test_validate_javascript_invalid() {
    // Missing closing brace
    let script = r#"
        let a = 1;
        if (a == 1) {
            return a;
    "#;
    let errors = validate_javascript(script);
    assert!(!errors.is_empty(), "Invalid script should produce errors");
    assert!(
        errors[0].contains("Syntax Error"),
        "Error message should mention Syntax Error, got: {}",
        errors[0]
    );
}
