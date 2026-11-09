use super::{Budget, MAX_JSON, MEMORY_LIMIT, error};
use rquickjs::{Context, Function, Object, Runtime};

pub(super) fn execute(
    source: &str,
    project: &str,
    context: &str,
    budget: &Budget,
) -> Result<String, String> {
    let runtime = Runtime::new().map_err(error)?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(512 * 1024);
    let check = budget.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || check.check().is_err())));
    let vm = Context::full(&runtime).map_err(error)?;
    vm.with(|ctx| {
        let execute = || -> rquickjs::Result<String> {
            let peel = Object::new(ctx.clone())?;
            peel.set("apiVersion", 1)?;
            peel.set("project", ctx.json_parse(project)?)?;
            peel.set("context", ctx.json_parse(context)?)?;
            peel.set(
                "newId",
                Function::new(ctx.clone(), || peel_document::Id::new().to_string())?,
            )?;
            let output = budget.clone();
            peel.set(
                "log",
                Function::new(ctx.clone(), move |text: String| output.log(text))?,
            )?;
            ctx.globals().set("peel", peel.clone())?;
            // Keep a private reference; replacing the global cannot swap the host object.
            ctx.eval::<(), _>(source)?;
            let value: rquickjs::Value = peel.get("project")?;
            let result = ctx.json_stringify(value)?.ok_or(rquickjs::Error::Unknown)?;
            result.to_string()
        };
        execute()
            .map_err(|e| {
                if e.is_exception() {
                    let exception = ctx.catch();
                    // Exception::from_value also retains stack traces and source line numbers.
                    if let Ok(exception) = rquickjs::Exception::from_value(exception.clone()) {
                        return format!("{exception}");
                    }
                    return format!("JavaScript exception: {exception:?}");
                }
                error(e)
            })
            .and_then(|json| {
                if json.len() > MAX_JSON {
                    Err("Script result exceeds 16 MiB".into())
                } else {
                    Ok(json)
                }
            })
    })
}
