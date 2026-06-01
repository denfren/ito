//! Script arguments injected as globals before execution.
//!
//! Mirrors the other scripting modules: instead of registering a Rhai
//! module on the engine, [`register`] attaches the typed args to the
//! script's [`Scope`] as plain global variables.

use rhai::Scope;

/// A typed script argument to inject as a global before script execution.
#[derive(Debug, Clone)]
pub enum ScriptArg {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

/// Attach the given `(name, value)` args to `scope` as global variables.
pub fn register(scope: &mut Scope, args: &[(String, ScriptArg)]) {
    for (name, val) in args {
        match val {
            ScriptArg::String(s) => scope.push(name.clone(), s.clone()),
            ScriptArg::Int(i) => scope.push(name.clone(), *i),
            ScriptArg::Float(f) => scope.push(name.clone(), *f),
            ScriptArg::Bool(b) => scope.push(name.clone(), *b),
        };
    }
}
