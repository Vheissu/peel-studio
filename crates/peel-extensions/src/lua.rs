use super::{Budget, MEMORY_LIMIT, error};
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, VmState};

pub(super) fn execute(
    source: &str,
    project: &str,
    context: &str,
    budget: &Budget,
) -> Result<String, String> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(error)?;
    lua.set_memory_limit(MEMORY_LIMIT).map_err(error)?;
    // Base library comes with every state; remove its file/code-loading entry points.
    for key in ["dofile", "loadfile", "load", "collectgarbage", "print"] {
        lua.globals().set(key, mlua::Value::Nil).map_err(error)?;
    }
    let check = budget.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            check.check().map_err(mlua::Error::external)?;
            Ok(VmState::Continue)
        },
    )
    .map_err(error)?;
    let execute = || -> mlua::Result<String> {
        let peel = lua.create_table()?;
        peel.set("apiVersion", 1)?;
        let project: serde_json::Value =
            serde_json::from_str(project).map_err(mlua::Error::external)?;
        let context: serde_json::Value =
            serde_json::from_str(context).map_err(mlua::Error::external)?;
        peel.set("project", lua.to_value(&project)?)?;
        peel.set("context", lua.to_value(&context)?)?;
        peel.set("null", lua.null())?;
        peel.set(
            "array",
            lua.create_function(|lua, ()| {
                let table = lua.create_table()?;
                table.set_metatable(Some(lua.array_metatable()))?;
                Ok(table)
            })?,
        )?;
        peel.set(
            "newId",
            lua.create_function(|_, ()| Ok(peel_document::Id::new().to_string()))?,
        )?;
        let output = budget.clone();
        peel.set(
            "log",
            lua.create_function(move |_, text: String| {
                output.log(text);
                Ok(())
            })?,
        )?;
        lua.globals().set("peel", peel.clone())?;
        lua.load(source)
            .set_name("script.lua")
            .set_mode(mlua::chunk::ChunkMode::Text)
            .exec()?;
        let value: Table = peel.get("project")?;
        let value = mlua::Value::Table(value);
        check_json(&value, budget, &mut 0, &mut 0, 0)?;
        let project: serde_json::Value = lua.from_value(value)?;
        serde_json::to_string(&project).map_err(mlua::Error::external)
    };
    execute().map_err(error)
}

// Validate the expanded tree before serde allocates host memory. Repeated table
// references can otherwise amplify a tiny Lua heap into a huge Rust value.
fn check_json(
    value: &mlua::Value,
    budget: &Budget,
    nodes: &mut usize,
    bytes: &mut usize,
    depth: usize,
) -> mlua::Result<()> {
    *nodes += 1;
    if *nodes > 2_000_000 || depth > 64 || *bytes > super::MAX_JSON {
        return Err(mlua::Error::external(
            "Lua result is too large or too deeply nested",
        ));
    }
    budget.check().map_err(mlua::Error::external)?;
    match value {
        mlua::Value::Table(t) => {
            for pair in t.pairs::<mlua::Value, mlua::Value>() {
                let (key, value) = pair?;
                check_json(&key, budget, nodes, bytes, depth + 1)?;
                check_json(&value, budget, nodes, bytes, depth + 1)?;
            }
        }
        mlua::Value::String(s) => *bytes += s.as_bytes().len(),
        _ => {}
    }
    Ok(())
}
