//! `tablefmt` module: render a list of rows as an aligned text table.
//!
//! A pure formatter over [`comfy_table`] -- no filesystem, no ambient state,
//! so it registers the same way in all four binaries. Named `tablefmt`
//! rather than `table`, which is Luau's own built-in library
//! (`table.insert`/`table.concat`/...); this module does not touch it.
//!
//! ```lua
//! local t = tablefmt.new({"name", "age"})
//! t:headers({"Name", "Age"})
//! t:rows({
//!     {name = "Ada", age = 36},
//!     {name = "Grace", age = 85},
//! })
//! t:align("age", tablefmt.RIGHT)
//! print(t:to_string())
//! ```
//!
//! `tablefmt.new(columns)` takes the column *keys*: the names a row's data is
//! looked up by, and the default header text until [`:headers()`][headers] or
//! [`:header()`][header] says otherwise. A row is either keyed by those names
//! (`{name = "Ada", age = 36}`) or plain positional values (`{"Ada", 36}`),
//! matched against the declared column order -- both forms are accepted on
//! the same table, since a script assembling rows from a query result and a
//! script building them by hand want different shapes.
//!
//! [headers]: Builder::headers
//! [header]: Builder::header
//!
//! `t:headers(nil)` omits the header row entirely, rather than setting it to
//! blank text -- comfy-table simply skips reserving a header line when
//! `set_header` is never called.
//!
//! Cell content is rendered with Lua's own `tostring` (so a pre-colored
//! string from `("x"):red()` passes through verbatim). Width measurement is
//! ANSI-aware (`comfy_table`'s `custom_styling` feature), so a colored cell
//! does not throw off column alignment by having its escape codes counted as
//! visible characters.
//!
//! Presets are the string constants `comfy_table::presets` uses
//! (`tablefmt.ASCII_FULL`, `tablefmt.UTF8_FULL_CONDENSED`, `tablefmt.NOTHING`,
//! ...), passed to [`:preset()`][Builder::preset]. Alignment is
//! `tablefmt.LEFT`, `tablefmt.CENTER`, `tablefmt.RIGHT`, passed to
//! [`:align()`][Builder::align].
//!
//! `:to_string([width])` and `:print([width])` are the two terminals. Without
//! a width, comfy-table's own terminal-size detection applies (this module
//! keeps comfy-table's default `tty` feature for that); with one, columns are
//! arranged to fit it.

use std::cell::RefCell;
use std::rc::Rc;

use comfy_table::{Cell, CellAlignment, ContentArrangement, Table as ComfyTable, TableStyle};
use mlua::{Lua, UserData, UserDataMethods, Value};

type Result<T> = mlua::Result<T>;

/// One row's cells, resolved from either a column-keyed or positional Lua
/// table at the time [`Builder::row`] is called, so later column/header
/// edits don't retroactively reshuffle it.
struct Row {
    cells: Vec<String>,
    /// Per-cell alignment overrides set via `:align(row, col, ...)`.
    alignment: Vec<Option<CellAlignment>>,
}

struct State {
    columns: Vec<String>,
    headers: Option<Vec<String>>,
    rows: Vec<Row>,
    column_alignment: Vec<Option<CellAlignment>>,
    preset: TableStyle,
}

impl State {
    fn column_index(&self, col: &Value) -> Result<usize> {
        match col {
            Value::Integer(n) => {
                let i = usize::try_from(*n - 1).map_err(|_| {
                    mlua::Error::runtime(format!("tablefmt: column index must be >= 1, got {n}"))
                })?;
                if i >= self.columns.len() {
                    return Err(mlua::Error::runtime(format!(
                        "tablefmt: column index {n} out of range (table has {} columns)",
                        self.columns.len()
                    )));
                }
                Ok(i)
            }
            Value::String(s) => {
                let s = s.to_str()?;
                self.columns
                    .iter()
                    .position(|c| c == &*s)
                    .ok_or_else(|| mlua::Error::runtime(format!("tablefmt: no column named {s:?}")))
            }
            other => Err(mlua::Error::runtime(format!(
                "tablefmt: column must be a number or string, got {}",
                other.type_name()
            ))),
        }
    }

    /// Resolve a row's data (keyed by column name, or positional) into one
    /// cell string per declared column.
    fn resolve_row(&self, data: &mlua::Table) -> Result<Vec<String>> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let by_name: Value = data.get(key.as_str())?;
                let v = if matches!(by_name, Value::Nil) {
                    data.get(i + 1)?
                } else {
                    by_name
                };
                Ok(cell_text(&v))
            })
            .collect()
    }
}

/// Render a Lua value the way `tostring` would; `nil` is the empty cell.
fn cell_text(v: &Value) -> String {
    match v {
        Value::Nil => String::new(),
        other => other
            .to_string()
            .unwrap_or_else(|e| format!("<error: {e}>")),
    }
}

/// The `tablefmt.new(...)` handle. `Rc<RefCell<State>>` so chained methods
/// share one builder, the way `fs.finder()` shares its state across
/// `:name()`, `:ext()`, ....
#[derive(Clone)]
pub struct Builder(Rc<RefCell<State>>);

impl Builder {
    fn build(&self, width: Option<u16>) -> ComfyTable {
        let state = self.0.borrow();
        let mut table = ComfyTable::new();
        table.load_style(state.preset);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        if let Some(w) = width {
            table.set_width(w);
        }

        if let Some(headers) = &state.headers {
            table.set_header(headers.iter().map(Cell::new));
        }
        for (i, alignment) in state.column_alignment.iter().enumerate() {
            if let (Some(alignment), Some(column)) = (alignment, table.column_mut(i)) {
                column.set_cell_alignment(*alignment);
            }
        }

        for row in &state.rows {
            let cells = row.cells.iter().zip(&row.alignment).map(|(text, align)| {
                let mut cell = Cell::new(text);
                if let Some(align) = align {
                    cell = cell.set_alignment(*align);
                }
                cell
            });
            table.add_row(cells);
        }

        table
    }
}

impl UserData for Builder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // t:preset(name) -> t
        methods.add_method("preset", |_, t, name: String| {
            t.0.borrow_mut().preset = preset_of(&name)?;
            Ok(t.clone())
        });

        // t:headers(names) -> t
        // t:headers(nil) -> t (omit the header row)
        methods.add_method("headers", |_, t, names: Option<Vec<String>>| {
            let mut state = t.0.borrow_mut();
            if let Some(names) = &names {
                if names.len() != state.columns.len() {
                    return Err(mlua::Error::runtime(format!(
                        "tablefmt:headers: expected {} names, got {}",
                        state.columns.len(),
                        names.len()
                    )));
                }
            }
            state.headers = names;
            Ok(t.clone())
        });

        // t:header(col, pretty_name) -> t
        methods.add_method("header", |_, t, (col, name): (Value, String)| {
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            match &mut state.headers {
                Some(headers) => headers[i] = name,
                None => {
                    return Err(mlua::Error::runtime(
                        "tablefmt:header: headers were omitted via headers(nil)",
                    ));
                }
            }
            Ok(t.clone())
        });

        // t:row(data) -> t
        methods.add_method("row", |_, t, data: mlua::Table| {
            let mut state = t.0.borrow_mut();
            let cells = state.resolve_row(&data)?;
            let n = state.columns.len();
            state.rows.push(Row {
                cells,
                alignment: vec![None; n],
            });
            Ok(t.clone())
        });

        // t:rows(data) -> t
        methods.add_method("rows", |_, t, data: Vec<mlua::Table>| {
            let mut state = t.0.borrow_mut();
            let n = state.columns.len();
            for row in data {
                let cells = state.resolve_row(&row)?;
                state.rows.push(Row {
                    cells,
                    alignment: vec![None; n],
                });
            }
            Ok(t.clone())
        });

        // t:align(col, alignment) -> t
        // t:align(row, col, alignment) -> t
        methods.add_method("align", |_, t, args: mlua::Variadic<Value>| {
            let mut state = t.0.borrow_mut();
            match args.len() {
                2 => {
                    let alignment = alignment_of(&args[1])?;
                    let i = state.column_index(&args[0])?;
                    state.column_alignment[i] = Some(alignment);
                }
                3 => {
                    let alignment = alignment_of(&args[2])?;
                    let i = state.column_index(&args[1])?;
                    let row_n = match &args[0] {
                        Value::Integer(n) => usize::try_from(*n - 1).map_err(|_| {
                            mlua::Error::runtime(format!(
                                "tablefmt:align: row index must be >= 1, got {n}"
                            ))
                        })?,
                        other => {
                            return Err(mlua::Error::runtime(format!(
                                "tablefmt:align: row must be a number, got {}",
                                other.type_name()
                            )));
                        }
                    };
                    let row_count = state.rows.len();
                    let row = state.rows.get_mut(row_n).ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "tablefmt:align: row index {} out of range ({row_count} rows)",
                            row_n + 1,
                        ))
                    })?;
                    row.alignment[i] = Some(alignment);
                }
                n => {
                    return Err(mlua::Error::runtime(format!(
                        "tablefmt:align: expected 2 or 3 arguments, got {n}"
                    )));
                }
            }
            Ok(t.clone())
        });

        // t:to_string([width]) -> string
        methods.add_method("to_string", |_, t, width: Option<u16>| {
            Ok(t.build(width).to_string())
        });

        // t:print([width]) -> ()
        methods.add_method("print", |_, t, width: Option<u16>| {
            println!("{}", t.build(width));
            Ok(())
        });
    }
}

fn alignment_of(v: &Value) -> Result<CellAlignment> {
    match v {
        Value::String(s) => match &*s.to_str()? {
            "left" => Ok(CellAlignment::Left),
            "center" => Ok(CellAlignment::Center),
            "right" => Ok(CellAlignment::Right),
            other => Err(mlua::Error::runtime(format!(
                "tablefmt: unknown alignment {other:?} (use tablefmt.LEFT/CENTER/RIGHT)"
            ))),
        },
        other => Err(mlua::Error::runtime(format!(
            "tablefmt: alignment must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn preset_of(name: &str) -> Result<TableStyle> {
    use comfy_table::presets::*;
    Ok(match name {
        "ASCII_FULL" => ASCII_FULL,
        "ASCII_FULL_CONDENSED" => ASCII_FULL_CONDENSED,
        "ASCII_NO_BORDERS" => ASCII_NO_BORDERS,
        "ASCII_BORDERS_ONLY" => ASCII_BORDERS_ONLY,
        "ASCII_BORDERS_ONLY_CONDENSED" => ASCII_BORDERS_ONLY_CONDENSED,
        "ASCII_HORIZONTAL_ONLY" => ASCII_HORIZONTAL_ONLY,
        "ASCII_MARKDOWN" => ASCII_MARKDOWN,
        "UTF8_FULL" => UTF8_FULL,
        "UTF8_FULL_CONDENSED" => UTF8_FULL_CONDENSED,
        "UTF8_NO_BORDERS" => UTF8_NO_BORDERS,
        "UTF8_BORDERS_ONLY" => UTF8_BORDERS_ONLY,
        "UTF8_HORIZONTAL_ONLY" => UTF8_HORIZONTAL_ONLY,
        "NOTHING" => NOTHING,
        other => {
            return Err(mlua::Error::runtime(format!(
                "tablefmt: unknown preset {other:?}"
            )));
        }
    })
}

/// Register the `tablefmt` module on `lua`.
pub fn register(lua: &Lua) -> Result<()> {
    let module = lua.create_table()?;

    for name in [
        "ASCII_FULL",
        "ASCII_FULL_CONDENSED",
        "ASCII_NO_BORDERS",
        "ASCII_BORDERS_ONLY",
        "ASCII_BORDERS_ONLY_CONDENSED",
        "ASCII_HORIZONTAL_ONLY",
        "ASCII_MARKDOWN",
        "UTF8_FULL",
        "UTF8_FULL_CONDENSED",
        "UTF8_NO_BORDERS",
        "UTF8_BORDERS_ONLY",
        "UTF8_HORIZONTAL_ONLY",
        "NOTHING",
    ] {
        module.set(name, name)?;
    }
    module.set("LEFT", "left")?;
    module.set("CENTER", "center")?;
    module.set("RIGHT", "right")?;

    // tablefmt.new(columns) -> Builder
    module.set(
        "new",
        lua.create_function(|_, columns: Vec<String>| {
            if columns.is_empty() {
                return Err(mlua::Error::runtime(
                    "tablefmt.new: at least one column is required",
                ));
            }
            let n = columns.len();
            let headers = Some(columns.clone());
            Ok(Builder(Rc::new(RefCell::new(State {
                columns,
                headers,
                rows: Vec::new(),
                column_alignment: vec![None; n],
                preset: preset_of("UTF8_FULL")?,
            }))))
        })?,
    )?;

    lua.globals().set("tablefmt", module)?;
    Ok(())
}
