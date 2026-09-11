//! `fmt.table`: render a list of rows as an aligned text table.
//!
//! A pure formatter over [`comfy_table`] -- no filesystem, no ambient state,
//! so it registers the same way in all three binaries and its output does not
//! depend on the terminal: without a width the table has its natural width
//! (no wrapping), with one the columns are arranged to fit it. Named
//! `fmt.table` rather than `table`, which is Luau's own built-in library.
//!
//! ```luau
//! local t = fmt.table.new({"name", "age"})
//! t:headers({"Name", "Age"})
//! t:rows({
//!     {name = "Ada", age = 36},
//!     {name = "Grace", age = 85},
//! })
//! t:align("age", fmt.table.RIGHT)
//! print(t:to_string())
//! ```
//!
//! `fmt.table.new(columns)` takes the column *keys*: the names a row's data is
//! looked up by, and the default header text until [`:headers()`][headers] or
//! [`:header()`][header] says otherwise. A row is either keyed by those names
//! (`{name = "Ada", age = 36}`) or plain positional values (`{"Ada", 36}`),
//! matched against the declared column order -- both forms are accepted on
//! the same table. A row is **validated**: an unknown key, a positional value
//! beyond the last column, or an empty row raises.
//!
//! `t:headers(nil)` omits the header row entirely; `t:headers()` with no
//! arguments raises rather than doing so by accident.
//!
//! A cell (or header) is a string, number, boolean or `nil` (empty); anything
//! else raises. A pre-colored string from `("x"):red()` passes through verbatim.
//! Width measurement is ANSI-aware and is the same Unicode width `fmt.width`
//! reports (`unicode-width`).
//!
//! Presets are the string constants of `fmt.table` (`fmt.table.UTF8_FULL`, …,
//! one list, [`PRESETS`]), passed to [`:preset()`][Builder::preset]; the two
//! modifiers `ROUND_CORNERS` and `SOLID_INNER_BORDERS` go to `:modifier()`.
//! Alignment is `fmt.table.LEFT`, `CENTER`, `RIGHT`.
//!
//! `:to_string([width])` (also `tostring(t)`) is the terminal. There is no
//! `:print`: use `print(t:to_string())`, which goes through the binary's own
//! stdout handling.

use std::cell::RefCell;
use std::rc::Rc;

use comfy_table::{
    Cell, CellAlignment, ColumnConstraint, ContentArrangement, Table as ComfyTable, TableStyle,
    Width,
};
use mlua::{Lua, MetaMethod, Table, UserData, UserDataMethods, Value};

type Result<T> = mlua::Result<T>;

/// The one preset list: the constant's name and its style.
const PRESETS: [(&str, TableStyle); 13] = {
    use comfy_table::presets::*;
    [
        ("ASCII_FULL", ASCII_FULL),
        ("ASCII_FULL_CONDENSED", ASCII_FULL_CONDENSED),
        ("ASCII_NO_BORDERS", ASCII_NO_BORDERS),
        ("ASCII_BORDERS_ONLY", ASCII_BORDERS_ONLY),
        ("ASCII_BORDERS_ONLY_CONDENSED", ASCII_BORDERS_ONLY_CONDENSED),
        ("ASCII_HORIZONTAL_ONLY", ASCII_HORIZONTAL_ONLY),
        ("ASCII_MARKDOWN", ASCII_MARKDOWN),
        ("UTF8_FULL", UTF8_FULL),
        ("UTF8_FULL_CONDENSED", UTF8_FULL_CONDENSED),
        ("UTF8_NO_BORDERS", UTF8_NO_BORDERS),
        ("UTF8_BORDERS_ONLY", UTF8_BORDERS_ONLY),
        ("UTF8_HORIZONTAL_ONLY", UTF8_HORIZONTAL_ONLY),
        ("NOTHING", NOTHING),
    ]
};

const MODIFIERS: [&str; 2] = ["ROUND_CORNERS", "SOLID_INNER_BORDERS"];

/// One row's cells, resolved from either a column-keyed or positional Luau
/// table at the time the row is added, so later column/header edits don't
/// retroactively reshuffle it.
struct Row {
    cells: Vec<String>,
    /// Per-cell alignment overrides set via `:align_cell(row, col, ...)`.
    alignment: Vec<Option<CellAlignment>>,
}

struct State {
    columns: Vec<String>,
    headers: Option<Vec<String>>,
    rows: Vec<Row>,
    column_alignment: Vec<Option<CellAlignment>>,
    header_alignment: Vec<Option<CellAlignment>>,
    column_width: Vec<Option<u16>>,
    padding: Vec<Option<(u16, u16)>>,
    preset: TableStyle,
    rounded: bool,
    solid_inner: bool,
}

fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

impl State {
    fn column_index(&self, col: &Value) -> Result<usize> {
        match col {
            Value::Integer(n) => {
                let i = usize::try_from(*n - 1)
                    .map_err(|_| err(format!("fmt.table: column index must be >= 1, got {n}")))?;
                if i >= self.columns.len() {
                    return Err(err(format!(
                        "fmt.table: column index {n} out of range (table has {} columns)",
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
                    .ok_or_else(|| err(format!("fmt.table: no column named {s:?}")))
            }
            other => Err(err(format!(
                "fmt.table: column must be a number or string, got {}",
                other.type_name()
            ))),
        }
    }

    /// Resolve a row's data (keyed by column name, or positional) into one
    /// cell string per declared column, rejecting anything that matches no
    /// column.
    fn resolve_row(&self, data: &Table) -> Result<Vec<String>> {
        let n = self.columns.len();
        let mut seen = 0;
        for pair in data.pairs::<Value, Value>() {
            let (k, _) = pair?;
            seen += 1;
            match &k {
                Value::String(s) => {
                    let s = s.to_str()?;
                    if !self.columns.iter().any(|c| c == &*s) {
                        return Err(err(format!("fmt.table: row has unknown column {s:?}")));
                    }
                }
                Value::Integer(i) if (1..=n as i64).contains(i) => {}
                Value::Integer(i) => {
                    return Err(err(format!(
                        "fmt.table: row has a value at position {i}, but the table has {n} columns"
                    )));
                }
                other => {
                    return Err(err(format!(
                        "fmt.table: row key must be a column name or position, got {}",
                        other.type_name()
                    )));
                }
            }
        }
        if seen == 0 {
            return Err(err("fmt.table: row is empty"));
        }
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
                text_of("fmt.table: cell", &v)
            })
            .collect()
    }

    fn push_row(&mut self, data: &Table) -> Result<()> {
        let cells = self.resolve_row(data)?;
        let n = self.columns.len();
        self.rows.push(Row {
            cells,
            alignment: vec![None; n],
        });
        Ok(())
    }
}

/// A cell/header value as text: strings, numbers and booleans verbatim (numbers
/// as `tostring` renders them), `nil` empty; anything else raises.
fn text_of(ctx: &str, v: &Value) -> Result<String> {
    match v {
        Value::Nil => Ok(String::new()),
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Integer(_) | Value::Number(_) | Value::Boolean(_) => Ok(v.to_string()?),
        other => Err(err(format!(
            "{ctx}: expected a string, number, boolean or nil, got {}",
            other.type_name()
        ))),
    }
}

fn width_arg(w: Option<f64>) -> Result<Option<u16>> {
    match w {
        None => Ok(None),
        Some(w) => (w.fract() == 0.0 && (0.0..=65535.0).contains(&w))
            .then_some(w as u16)
            .filter(|w| *w >= 1)
            .map(Some)
            .ok_or_else(|| err(format!("fmt.table: width must be 1..=65535, got {w}"))),
    }
}

/// The `fmt.table.new(...)` handle. `Rc<RefCell<State>>` so chained methods
/// share one builder, the way `fs.finder()` shares its state across
/// `:name()`, `:ext()`, ....
#[derive(Clone)]
pub struct Builder(Rc<RefCell<State>>);

impl Builder {
    fn build(&self, width: Option<u16>) -> ComfyTable {
        let state = self.0.borrow();
        let mut table = ComfyTable::new();
        let mut style = state.preset;
        if state.rounded {
            style = style.with_rounded_corners();
        }
        if state.solid_inner {
            style = style.with_solid_inner_borders();
        }
        table.load_style(style);
        // No width: the natural one, so the output does not depend on the
        // terminal this happens to run in.
        match width {
            Some(w) => {
                table.set_content_arrangement(ContentArrangement::Dynamic);
                table.set_width(w);
            }
            None => {
                table.set_content_arrangement(ContentArrangement::Disabled);
            }
        }

        if let Some(headers) = &state.headers {
            table.set_header(headers.iter().enumerate().map(|(i, h)| {
                let mut cell = Cell::new(h);
                if let Some(a) = state.header_alignment[i] {
                    cell = cell.set_alignment(a);
                }
                cell
            }));
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
        // Columns exist only once cells do, so column settings go last.
        for i in 0..state.columns.len() {
            if let Some(column) = table.column_mut(i) {
                if let Some(a) = state.column_alignment[i] {
                    column.set_cell_alignment(a);
                }
                if let Some(w) = state.column_width[i] {
                    column.set_constraint(ColumnConstraint::Absolute(Width::Fixed(w)));
                }
                if let Some(p) = state.padding[i] {
                    column.set_padding(p);
                }
            }
        }

        table
    }
}

impl UserData for Builder {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // t:preset(name) -> t
        methods.add_method("preset", |_, t, name: String| {
            let (_, style) = PRESETS
                .iter()
                .find(|(n, _)| *n == name)
                .ok_or_else(|| err(format!("fmt.table: unknown preset {name:?}")))?;
            t.0.borrow_mut().preset = *style;
            Ok(t.clone())
        });

        // t:modifier(name) -> t
        methods.add_method("modifier", |_, t, name: String| {
            let mut state = t.0.borrow_mut();
            match name.as_str() {
                "ROUND_CORNERS" => state.rounded = true,
                "SOLID_INNER_BORDERS" => state.solid_inner = true,
                other => return Err(err(format!("fmt.table: unknown modifier {other:?}"))),
            }
            Ok(t.clone())
        });

        // t:headers(names) -> t
        // t:headers(nil) -> t (omit the header row)
        // t:headers(a, b, ...) -> t (same, as varargs)
        methods.add_method("headers", |_, t, args: mlua::Variadic<Value>| {
            let names = match args.first() {
                None => {
                    return Err(err(
                        "fmt.table:headers: no names given (use headers(nil) to omit the header row)",
                    ));
                }
                Some(Value::Nil) => None,
                _ => Some(text_list("fmt.table:headers", &args)?),
            };
            let mut state = t.0.borrow_mut();
            if let Some(names) = &names {
                if names.len() != state.columns.len() {
                    return Err(err(format!(
                        "fmt.table:headers: expected {} names, got {}",
                        state.columns.len(),
                        names.len()
                    )));
                }
            }
            state.headers = names;
            Ok(t.clone())
        });

        // t:header(col, pretty_name) -> t
        methods.add_method("header", |_, t, (col, name): (Value, Value)| {
            let name = text_of("fmt.table:header", &name)?;
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            match &mut state.headers {
                Some(headers) => headers[i] = name,
                None => {
                    return Err(err("fmt.table:header: headers were omitted via headers(nil)"));
                }
            }
            Ok(t.clone())
        });

        // t:row(data) -> t
        // t:row(a, b, ...) -> t (positional, as varargs)
        methods.add_method("row", |lua, t, args: mlua::Variadic<Value>| {
            let data = match args.as_slice() {
                [Value::Table(data)] => data.clone(),
                _ => lua.create_sequence_from(args.iter().cloned())?,
            };
            t.0.borrow_mut().push_row(&data)?;
            Ok(t.clone())
        });

        // t:rows(data) -> t
        methods.add_method("rows", |_, t, data: Value| {
            let Value::Table(list) = data else {
                return Err(err(format!(
                    "fmt.table:rows: expected a list of rows, got {}",
                    data.type_name()
                )));
            };
            let len = list.raw_len();
            if list.pairs::<Value, Value>().count() != len {
                return Err(err("fmt.table:rows: expected a list of rows, got a map"));
            }
            let mut state = t.0.borrow_mut();
            for (i, row) in list.sequence_values::<Value>().enumerate() {
                match row? {
                    Value::Table(row) => state.push_row(&row)?,
                    other => {
                        return Err(err(format!(
                            "fmt.table:rows: row {} must be a table, got {}",
                            i + 1,
                            other.type_name()
                        )));
                    }
                }
            }
            Ok(t.clone())
        });

        // t:align(col, alignment) -> t
        methods.add_method("align", |_, t, (col, a): (Value, Value)| {
            let a = alignment_of(&a)?;
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            state.column_alignment[i] = Some(a);
            Ok(t.clone())
        });

        // t:align_header(col, alignment) -> t
        methods.add_method("align_header", |_, t, (col, a): (Value, Value)| {
            let a = alignment_of(&a)?;
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            state.header_alignment[i] = Some(a);
            Ok(t.clone())
        });

        // t:align_cell(row, col, alignment) -> t
        methods.add_method("align_cell", |_, t, (row, col, a): (i64, Value, Value)| {
            let a = alignment_of(&a)?;
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            let row_count = state.rows.len();
            let r = usize::try_from(row - 1)
                .ok()
                .filter(|r| *r < row_count)
                .ok_or_else(|| {
                    err(format!(
                        "fmt.table:align_cell: row index {row} out of range ({row_count} rows)"
                    ))
                })?;
            state.rows[r].alignment[i] = Some(a);
            Ok(t.clone())
        });

        // t:column_width(col, n) -> t (fixed content width)
        methods.add_method("column_width", |_, t, (col, n): (Value, i64)| {
            let w = width_arg(Some(n as f64))?;
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            state.column_width[i] = w;
            Ok(t.clone())
        });

        // t:padding(col, left, right) -> t
        methods.add_method("padding", |_, t, (col, l, r): (Value, u16, u16)| {
            let mut state = t.0.borrow_mut();
            let i = state.column_index(&col)?;
            state.padding[i] = Some((l, r));
            Ok(t.clone())
        });

        // t:to_string([width]) -> string
        methods.add_method("to_string", |_, t, width: Option<f64>| {
            Ok(t.build(width_arg(width)?).to_string())
        });

        methods.add_meta_method(MetaMethod::ToString, |_, t, ()| Ok(t.build(None).to_string()));
    }
}

/// A list of cell texts given either as one table or as varargs.
fn text_list(ctx: &str, args: &[Value]) -> Result<Vec<String>> {
    let items: Vec<Value> = match args {
        [Value::Table(t)] => t.sequence_values::<Value>().collect::<Result<_>>()?,
        _ => args.to_vec(),
    };
    items
        .iter()
        .map(|v| match v {
            Value::Nil => Err(err(format!("{ctx}: names must not be nil"))),
            v => text_of(ctx, v),
        })
        .collect()
}

fn alignment_of(v: &Value) -> Result<CellAlignment> {
    match v {
        Value::String(s) => match &*s.to_str()? {
            "left" => Ok(CellAlignment::Left),
            "center" => Ok(CellAlignment::Center),
            "right" => Ok(CellAlignment::Right),
            other => Err(err(format!(
                "fmt.table: unknown alignment {other:?} (use fmt.table.LEFT/CENTER/RIGHT)"
            ))),
        },
        other => Err(err(format!(
            "fmt.table: alignment must be a string, got {}",
            other.type_name()
        ))),
    }
}

/// Register `fmt.table` (constants and `new`) on the `fmt` table.
pub fn register(lua: &Lua, fmt: &Table) -> Result<()> {
    let module = lua.create_table()?;

    for (name, _) in PRESETS {
        module.set(name, name)?;
    }
    for name in MODIFIERS {
        module.set(name, name)?;
    }
    module.set("LEFT", "left")?;
    module.set("CENTER", "center")?;
    module.set("RIGHT", "right")?;

    // fmt.table.new(columns) -> Builder
    // fmt.table.new(a, b, ...) -> Builder (same, as varargs)
    module.set(
        "new",
        lua.create_function(|_, args: mlua::Variadic<Value>| {
            let columns = text_list("fmt.table.new", &args)?;
            if columns.is_empty() {
                return Err(err("fmt.table.new: at least one column is required"));
            }
            let n = columns.len();
            let headers = Some(columns.clone());
            Ok(Builder(Rc::new(RefCell::new(State {
                columns,
                headers,
                rows: Vec::new(),
                column_alignment: vec![None; n],
                header_alignment: vec![None; n],
                column_width: vec![None; n],
                padding: vec![None; n],
                preset: comfy_table::presets::UTF8_FULL,
                rounded: false,
                solid_inner: false,
            }))))
        })?,
    )?;

    fmt.set("table", module)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua() -> Lua {
        let lua = Lua::new();
        super::super::register(&lua).unwrap();
        lua
    }

    fn ev(lua: &Lua, src: &str) -> String {
        lua.load(src).eval().unwrap()
    }

    #[test]
    fn renders_keyed_and_positional_rows() {
        let lua = lua();
        let out = ev(
            &lua,
            r#"
            local t = fmt.table.new({"name", "age"}):preset(fmt.table.ASCII_FULL)
            t:headers({"Name", "Age"})
            t:rows({{name = "Ada", age = 36}, {"Grace", 85}})
            t:align("age", fmt.table.RIGHT)
            return t:to_string()"#,
        );
        assert_eq!(
            out,
            "\
+-------+-----+
| Name  | Age |
+=============+
| Ada   |  36 |
|-------+-----|
| Grace |  85 |
+-------+-----+"
        );
    }

    #[test]
    fn natural_width_is_pure_and_tostring_works() {
        let lua = lua();
        let out = ev(
            &lua,
            r#"return tostring(fmt.table.new("a"):row("a very long cell that would wrap"))"#,
        );
        assert!(out.contains("a very long cell that would wrap"), "{out}");
    }

    #[test]
    fn width_wraps() {
        let lua = lua();
        let out = ev(
            &lua,
            r#"return fmt.table.new("a"):row("a very long cell that would wrap"):to_string(14)"#,
        );
        assert!(out.lines().all(|l| l.chars().count() <= 14), "{out}");
    }

    #[test]
    fn bad_rows_raise() {
        let lua = lua();
        for src in [
            r#"fmt.table.new("a", "b"):row({a = 1, nope = 2})"#,
            r#"fmt.table.new("a", "b"):row(1, 2, 3)"#,
            r#"fmt.table.new("a"):row({})"#,
            r#"fmt.table.new("a"):row(1, {})"#,
            r#"fmt.table.new("a"):rows({1})"#,
            r#"fmt.table.new("a"):rows({x = {1}})"#,
            r#"fmt.table.new("a"):row({a = print})"#,
            "fmt.table.new('a'):headers()",
            "fmt.table.new('a'):to_string(0)",
            "fmt.table.new('a'):to_string(-3)",
            "fmt.table.new('a'):to_string(2.5)",
        ] {
            assert!(lua.load(src).exec().is_err(), "{src}");
        }
    }

    #[test]
    fn header_numbers_and_header_alignment() {
        let lua = lua();
        let out = ev(
            &lua,
            r#"return fmt.table.new("n"):preset(fmt.table.ASCII_MARKDOWN)
                :headers(7):row("abcdef"):align_header("n", fmt.table.RIGHT):to_string()"#,
        );
        assert!(out.contains("|      7 |"), "{out}");
    }

    #[test]
    fn colored_cells_do_not_skew_columns() {
        let _g = super::super::TEST_LOCK.lock().unwrap();
        super::super::ansi::reset();
        let lua = lua();
        let out = ev(
            &lua,
            r#"fmt.colors(true)
            return fmt.table.new("a"):preset(fmt.table.ASCII_MARKDOWN):row(("ok"):green()):to_string()"#,
        );
        let plain = super::super::text::strip(&out);
        let widths: Vec<_> = plain.lines().map(|l| l.chars().count()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{plain}");
    }

    #[test]
    fn nothing_preset_keeps_column_padding() {
        let lua = lua();
        let out = ev(
            &lua,
            r#"return fmt.table.new("a"):preset(fmt.table.NOTHING):headers(nil):row("x"):to_string()"#,
        );
        assert_eq!(out, " x ", "NOTHING has no borders but keeps the 1-char padding");
    }
}
