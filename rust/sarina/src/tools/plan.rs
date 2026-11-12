//! Go: `go/tools/plan.go`'s tools: `enter_plan_mode` and `exit_plan_mode`. They only ask for the change (metadata
//! `plan_action`); the agent owns the state, and exit_plan_mode submits the plan for the operator's decision
//! (testdata/parity/plantools.json). The plan-mode filter of the registry is `Registry::for_mode`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::goany::Any;
use crate::tools::files::{arg_str, fail};
use crate::tools::registry::{Context, Output, TOOL_ENTER_PLAN_MODE, TOOL_EXIT_PLAN_MODE};

/// The longest plan the model may submit, in characters.
pub const MAX_PLAN_CHARS: usize = 20000;

/// Whether plan mode is on, as the agent reports it through the context (Go's RuntimePlanActive).
pub type PlanActive = Arc<dyn Fn() -> bool + Send + Sync>;

fn plan_active(tc: &Context) -> Option<bool> {
    tc.plan_active.as_ref().map(|f| f())
}

/// Go's `enter_plan_mode` handler.
pub fn enter_plan_mode(_: &BTreeMap<String, Any>, tc: &Context) -> Output {
    if plan_active(tc) == Some(true) {
        return fail("already in plan mode".into());
    }
    Output {
        content: format!(
            "Plan mode is on. Only read-only tools are available. Investigate, then call {TOOL_EXIT_PLAN_MODE} with your plan; the operator \
             decides whether to approve it."
        ),
        metadata: BTreeMap::from([("plan_action".to_owned(), Any::String("enter".into()))]),
        error: None,
    }
}

/// Go's `exit_plan_mode` handler.
pub fn exit_plan_mode(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    if plan_active(tc) == Some(false) {
        return fail("not in plan mode, so there is no plan to submit".into());
    }
    let n = arg_str(args, "plan").chars().count();
    if n > MAX_PLAN_CHARS {
        return fail(format!("the plan is {n} characters; the limit is {MAX_PLAN_CHARS}. Shorten it and submit again"));
    }
    Output {
        content: "Plan submitted to the operator.".into(),
        metadata: BTreeMap::from([("plan_action".to_owned(), Any::String("exit".into())), ("plan_chars".to_owned(), Any::Int(n as i64))]),
        error: None,
    }
}

/// enter_plan_mode and exit_plan_mode with the Go build's definitions.
pub fn plan_tools() -> Vec<crate::tools::registry::Tool> {
    use crate::tools::defs::tool;
    vec![tool(TOOL_ENTER_PLAN_MODE, Arc::new(enter_plan_mode)), tool(TOOL_EXIT_PLAN_MODE, Arc::new(exit_plan_mode))]
}
