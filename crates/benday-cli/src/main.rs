use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use benday_core::ingest::{self, DataDoc};
use benday_core::{render, spec::Spec, theme, BarStyle, Marker, RenderOptions};

const EXAMPLES: &str = r#"Examples:
  echo '{"data":{"values":[{"m":"jan","v":3},{"m":"feb","v":7}]},"mark":"bar","encoding":{"x":{"field":"m"},"y":{"field":"v"}}}' | benday
  benday --spec-file chart.json --marker octant --theme lichtenstein

Spec (a strict Vega-Lite subset):
  { "data": { "values": [ {..row..}, ... ] },
    "mark": "bar" | "line" | "point" | "area",
    "encoding": {
      "x": { "field": str, "type"?: "quantitative"|"nominal"|"ordinal" },
      "y": { "field": str, "aggregate"?: "sum"|"mean"|"median"|"min"|"max"|"count" },
      "color"?: { "field": str }
    },
    "title"?: str, "width"?: cells, "height"?: cells }

Exit codes: 0 ok, 2 invalid spec, 3 data does not fit the encoding.
"#;

/// Crisp terminal charts from a Vega-Lite-style JSON spec. Built to be
/// called by AI agents: deterministic one-shot output, no TTY dependence,
/// machine-readable errors on stderr.
#[derive(Parser)]
#[command(name = "benday", version, after_help = EXAMPLES)]
struct Cli {
    /// Inline spec JSON (reads stdin when omitted)
    #[arg(long, value_name = "JSON")]
    spec: Option<String>,

    /// Read the spec JSON from a file
    #[arg(long, value_name = "PATH", conflicts_with = "spec")]
    spec_file: Option<std::path::PathBuf>,

    /// Plot area width in terminal cells (overrides spec.width; default 60)
    #[arg(long)]
    width: Option<usize>,

    /// Plot area height in terminal cells (overrides spec.height; default 10)
    #[arg(long)]
    height: Option<usize>,

    /// Sub-cell pixel style for line/point/area marks
    #[arg(long, value_enum, default_value_t = MarkerArg::Braille)]
    marker: MarkerArg,

    /// Bar fill: canvas dots (house style) or solid blocks (finer 8-levels/cell caps)
    #[arg(long, value_enum, default_value_t = BarStyleArg::Dots)]
    bar_style: BarStyleArg,

    /// Color theme: benday | lichtenstein | rotogravure
    #[arg(long, default_value = "benday")]
    theme: String,

    /// Disable ANSI colors (color is ON by default, even when piped)
    #[arg(long)]
    no_color: bool,

    /// Print render metadata JSON (domains, series colors, dropped rows) to stderr
    #[arg(long)]
    meta: bool,

    /// Print the compiled scene as JSON instead of rendering (UNSTABLE:
    /// debugging interface, format changes without notice)
    #[arg(long)]
    dump_scene: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum MarkerArg {
    Braille,
    Octant,
}

impl From<MarkerArg> for Marker {
    fn from(m: MarkerArg) -> Marker {
        match m {
            MarkerArg::Braille => Marker::Braille,
            MarkerArg::Octant => Marker::Octant,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum BarStyleArg {
    Dots,
    Blocks,
}

impl From<BarStyleArg> for BarStyle {
    fn from(b: BarStyleArg) -> BarStyle {
        match b {
            BarStyleArg::Dots => BarStyle::Dots,
            BarStyleArg::Blocks => BarStyle::Blocks,
        }
    }
}

fn fail(kind: &str, message: &str, code: u8) -> ExitCode {
    eprintln!(
        "{}",
        serde_json::json!({ "error": { "kind": kind, "message": message } })
    );
    ExitCode::from(code)
}

/// Parse a spec source string, formatting serde_path_to_error's path-precise
/// error the way the CLI has always surfaced it.
fn parse_spec(source: &str) -> Result<Spec, String> {
    let mut de = serde_json::Deserializer::from_str(source);
    serde_path_to_error::deserialize(&mut de).map_err(|e| {
        let path = e.path().to_string();
        let loc = if path == "." {
            String::new()
        } else {
            format!("at `{path}`: ")
        };
        format!(
            "{loc}{}; run `benday --help` for the supported spec shape",
            e.inner()
        )
    })
}

/// Heuristic for the "forgot --spec" case: stdin was supposed to be a spec but
/// smells like a piped data document — a bare array, or an object carrying
/// `columns`/`rows` yet no `mark`.
fn looks_like_data(source: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(source) {
        Ok(serde_json::Value::Array(_)) => true,
        Ok(serde_json::Value::Object(map)) => {
            (map.contains_key("columns") || map.contains_key("rows")) && !map.contains_key("mark")
        }
        _ => false,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // stdin's role is decided by where the spec came from: a --spec/--spec-file
    // flag means stdin carries the DATA document; otherwise stdin IS the spec.
    let (source, spec_from_flag) = if let Some(s) = &cli.spec {
        (s.clone(), true)
    } else if let Some(path) = &cli.spec_file {
        match std::fs::read_to_string(path) {
            Ok(s) => (s, true),
            Err(e) => return fail("spec", &format!("cannot read {}: {e}", path.display()), 2),
        }
    } else if std::io::stdin().is_terminal() {
        return fail(
            "spec",
            "no spec provided: pipe JSON to stdin, or use --spec / --spec-file (see --help)",
            2,
        );
    } else {
        let mut s = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut s) {
            return fail("spec", &format!("cannot read stdin: {e}"), 2);
        }
        (s, false)
    };

    let spec: Spec = match parse_spec(&source) {
        Ok(s) => s,
        Err(msg) => {
            // No spec flag means stdin was meant to be the spec; if it instead
            // looks like a data document, the user likely forgot --spec.
            if !spec_from_flag && looks_like_data(&source) {
                return fail(
                    "spec",
                    "stdin looks like a data document, not a spec; pass the spec via \
                     --spec '...' and keep the data on stdin",
                    2,
                );
            }
            return fail("spec", &msg, 2);
        }
    };

    // When the spec came from a flag, stdin (if any, and non-empty) is a data
    // document. Not read at all when stdin was already consumed as the spec.
    let stdin_doc: Option<DataDoc> = if spec_from_flag && !std::io::stdin().is_terminal() {
        let mut s = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut s) {
            return fail("data", &format!("cannot read stdin: {e}"), 3);
        }
        if s.trim().is_empty() {
            None
        } else {
            match ingest::parse_data_doc(&s) {
                Ok(doc) => Some(doc),
                Err(e) => return fail(e.kind(), &e.to_string(), 3),
            }
        }
    } else {
        None
    };

    let Some(theme) = theme::by_name(&cli.theme) else {
        return fail(
            "spec",
            &format!(
                "unknown theme \"{}\"; available themes: {}",
                cli.theme,
                theme::THEME_NAMES.join(", ")
            ),
            2,
        );
    };

    if cli.dump_scene {
        let copts = benday_core::compile::CompileOptions {
            width: cli.width,
            height: cli.height,
            theme,
        };
        let scene = ingest::resolve(&spec, stdin_doc)
            .and_then(|table| benday_core::compile::compile(&spec, &table, &copts));
        return match scene {
            Ok(scene) => {
                println!("{}", scene.to_json());
                ExitCode::SUCCESS
            }
            Err(e) => {
                let code = if e.kind() == "spec" { 2 } else { 3 };
                fail(e.kind(), &e.to_string(), code)
            }
        };
    }

    let opts = RenderOptions {
        width: cli.width,
        height: cli.height,
        marker: cli.marker.into(),
        bar_style: cli.bar_style.into(),
        theme,
        color: !cli.no_color,
    };

    match render(&spec, stdin_doc, &opts) {
        Ok(out) => {
            print!("{}", out.text);
            if cli.meta {
                eprintln!("{}", out.meta);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let code = if e.kind() == "spec" { 2 } else { 3 };
            fail(e.kind(), &e.to_string(), code)
        }
    }
}
