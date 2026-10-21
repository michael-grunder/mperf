use crate::{
    cli::{ListKind, Selection},
    profile::{Frame, Profile, Stack},
};
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek},
    path::Path,
    process::{Command, Stdio},
};

pub fn check_available() -> Result<()> {
    let output = Command::new("xcrun")
        .args(["--find", "xctrace"])
        .output()
        .context("xcrun is unavailable; recording/export requires macOS with full Xcode")?;
    ensure!(
        output.status.success(),
        "xctrace is unavailable. Install full Xcode and select it using xcode-select; Command Line Tools alone do not include Instruments. {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub fn list(kind: ListKind) -> Result<()> {
    check_available()?;
    let kind = match kind {
        ListKind::Templates => "templates",
        ListKind::Instruments => "instruments",
        ListKind::Devices => "devices",
    };
    let status = Command::new("xcrun")
        .args(["xctrace", "list", kind])
        .status()?;
    ensure!(status.success(), "xctrace list {kind} failed: {status}");
    Ok(())
}

fn export(path: &Path, query: Option<&str>) -> Result<String> {
    // Spool stdout to disk so a large XML export is not buffered twice by Command::output.
    let mut file = tempfile::tempfile()?;
    let mut command = Command::new("xcrun");
    command.args(["xctrace", "export", "--input"]).arg(path);
    if let Some(xpath) = query {
        command.args(["--xpath", xpath]);
    } else {
        command.arg("--toc");
    }
    let output = command
        .stdout(Stdio::from(file.try_clone()?))
        .output()
        .context("exporting xctrace data")?;
    ensure!(
        output.status.success(),
        "xctrace export failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    file.rewind()?;
    let mut xml = String::new();
    file.read_to_string(&mut xml)
        .context("xctrace output is not UTF-8 XML")?;
    Ok(xml)
}

pub fn load(path: &Path, selection: &Selection) -> Result<Profile> {
    ensure!(
        path.exists(),
        "input {} does not exist; record first or select an input (see 'mperf report help')",
        path.display()
    );
    if path.is_file() {
        ensure!(
            selection.run.is_none(),
            "--run only applies to trace bundles; an XML export already selects a run"
        );
        let xml =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let profile = parse(&xml, path.display().to_string(), None)?;
        if let Some(schema) = &selection.schema {
            ensure!(
                schema == &profile.schema,
                "XML schema is '{}', not requested '{}'",
                profile.schema,
                schema
            );
        }
        return Ok(profile);
    }
    check_available()?;
    eprintln!("mperf: exporting CPU samples from {} ...", path.display());
    let toc = export(path, None)?;
    let (run, schema) = select_table(&toc, selection)?;
    let xpath = format!("/trace-toc/run[@number='{run}']/data/table[@schema='{schema}']");
    let xml = export(path, Some(&xpath))?;
    parse(&xml, path.display().to_string(), Some(run))
}

fn select_table(toc: &str, selection: &Selection) -> Result<(u32, String)> {
    let document = Document::parse(toc).context("parsing xctrace table of contents")?;
    let runs: Vec<_> = document
        .descendants()
        .filter(|n| n.has_tag_name("run"))
        .filter_map(|n| {
            n.attribute("number")
                .and_then(|s| s.parse::<u32>().ok())
                .map(|id| (id, n))
        })
        .collect();
    let run = selection
        .run
        .or_else(|| runs.iter().map(|(id, _)| *id).max())
        .context("trace has no runs")?;
    let (_, node) = runs.iter().find(|(id, _)| *id == run).with_context(|| {
        format!(
            "run {run} not found; available runs: {:?}",
            runs.iter().map(|(id, _)| id).collect::<Vec<_>>()
        )
    })?;
    let schemas: Vec<_> = node
        .descendants()
        .filter(|n| n.has_tag_name("table"))
        .filter_map(|n| n.attribute("schema"))
        .collect();
    let schema = if let Some(schema) = &selection.schema {
        ensure!(
            schemas.contains(&schema.as_str()),
            "run {run} has no {schema} table; available: {}",
            schemas.join(", ")
        );
        schema.as_str()
    } else {
        ["time-profile", "cpu-profile"].into_iter().find(|s| schemas.contains(s))
            .with_context(|| format!("run {run} has no supported CPU sample table. Available: {}. Record with the Time Profiler template", schemas.join(", ")))?
    };
    Ok((run, schema.to_owned()))
}

struct References<'a, 'input> {
    nodes: HashMap<(&'input str, &'a str), Node<'a, 'input>>,
}

impl<'a, 'input> References<'a, 'input> {
    fn new(root: Node<'a, 'input>) -> Self {
        Self {
            nodes: root
                .descendants()
                .filter_map(|n| n.attribute("id").map(|id| ((n.tag_name().name(), id), n)))
                .collect(),
        }
    }
    fn resolve(&self, mut node: Node<'a, 'input>) -> Result<Node<'a, 'input>> {
        for _ in 0..64 {
            let Some(reference) = node.attribute("ref") else {
                return Ok(node);
            };
            node = *self
                .nodes
                .get(&(node.tag_name().name(), reference))
                .with_context(|| {
                    format!(
                        "unresolved XML reference {} ref={reference}",
                        node.tag_name().name()
                    )
                })?;
        }
        bail!("cyclic or excessively deep XML reference")
    }
    fn label(&self, node: Node<'a, 'input>) -> Result<String> {
        let node = self.resolve(node)?;
        Ok(node
            .attribute("name")
            .or_else(|| node.attribute("fmt"))
            .or_else(|| node.text())
            .unwrap_or("[unknown]")
            .trim()
            .to_owned())
    }
    fn frame(&self, node: Node<'a, 'input>) -> Result<Frame> {
        let node = self.resolve(node)?;
        let symbol = node
            .attribute("name")
            .or_else(|| node.attribute("fmt"))
            .or_else(|| node.attribute("addr"))
            .unwrap_or("[unknown]")
            .to_owned();
        let image = node
            .children()
            .find(|n| n.has_tag_name("binary") || n.has_tag_name("image"))
            .map(|n| self.label(n))
            .transpose()?
            .unwrap_or_else(|| "[unknown]".into());
        Ok(Frame { symbol, image })
    }
}

pub fn parse(xml: &str, source: String, run: Option<u32>) -> Result<Profile> {
    let document = Document::parse(xml).context("parsing CPU sample XML")?;
    let tables: Vec<_> = document
        .descendants()
        .filter(|n| n.has_tag_name("schema") && n.attribute("name").is_some())
        .collect();
    ensure!(
        tables.len() == 1,
        "expected exactly one exported CPU table with a schema; found {}. Export a single run/table with xctrace --xpath",
        tables.len()
    );
    let schema_node = tables[0];
    let schema = schema_node.attribute("name").unwrap_or_default();
    ensure!(
        ["time-profile", "cpu-profile"].contains(&schema),
        "unsupported table '{schema}'; report supports time-profile and cpu-profile CPU backtraces"
    );
    let root = schema_node.parent().context("schema has no table")?;
    let references = References::new(root);
    let rows: Vec<_> = root.children().filter(|n| n.has_tag_name("row")).collect();
    ensure!(
        !rows.is_empty(),
        "CPU table '{schema}' contains no samples; record longer or verify the target was active"
    );
    let mut profile = Profile {
        source,
        schema: schema.to_owned(),
        run,
        ..Profile::default()
    };
    for row in rows {
        add_row(&mut profile, row, &references)?;
    }
    ensure!(
        profile.total.is_finite() && profile.total > 0.0,
        "CPU table has no positive sample weight"
    );
    ensure!(
        !profile.stacks.is_empty(),
        "CPU table has {} samples but no usable backtraces; choose time-profile or cpu-profile, or record with Time Profiler",
        profile.samples
    );
    Ok(profile)
}

fn add_row(profile: &mut Profile, row: Node<'_, '_>, refs: &References<'_, '_>) -> Result<()> {
    let weight_node = row.children().find(|n| {
        matches!(
            n.tag_name().name(),
            "weight" | "cycle-weight" | "sample-weight"
        )
    });
    let (weight, unit) = if let Some(node) = weight_node {
        let node = refs.resolve(node)?;
        let value = node
            .text()
            .unwrap_or_default()
            .trim()
            .parse::<f64>()
            .context("invalid numeric sample weight")?;
        let unit = if node.has_tag_name("cycle-weight") {
            "cycles"
        } else {
            "ns"
        };
        (value, unit)
    } else {
        (1.0, "samples")
    };
    ensure!(
        weight.is_finite() && weight >= 0.0,
        "sample weight must be finite and nonnegative"
    );
    ensure!(
        profile.unit.is_empty() || profile.unit == unit,
        "inconsistent weight columns in CPU table"
    );
    unit.clone_into(&mut profile.unit);
    profile.samples += 1;
    profile.total += weight;
    let backtrace = row
        .children()
        .find(|n| matches!(n.tag_name().name(), "backtrace" | "tagged-backtrace"));
    let frames = if let Some(backtrace) = backtrace {
        refs.resolve(backtrace)?
            .children()
            .filter(|n| n.has_tag_name("frame"))
            .map(|n| refs.frame(n))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    if frames.is_empty() {
        profile.missing_stacks += 1;
        return Ok(());
    }
    let thread_node = row
        .children()
        .find(|n| n.has_tag_name("thread"))
        .map(|n| refs.resolve(n))
        .transpose()?;
    let process_node = row
        .children()
        .find(|n| n.has_tag_name("process"))
        .or_else(|| thread_node.and_then(|n| n.children().find(|c| c.has_tag_name("process"))));
    let process = process_node
        .map(|n| refs.resolve(n).map(process_name))
        .transpose()?
        .unwrap_or_else(|| "[unknown]".into());
    let thread = thread_node
        .map(|n| refs.label(n))
        .transpose()?
        .unwrap_or_else(|| "[unknown thread]".into());
    *profile
        .stacks
        .entry(Stack {
            process,
            thread,
            frames,
        })
        .or_default() += weight;
    Ok(())
}

fn process_name(node: Node<'_, '_>) -> String {
    let name = node
        .children()
        .find(|n| n.has_tag_name("name"))
        .and_then(|n| n.text())
        .or_else(|| node.attribute("name"));
    if let Some(name) = name {
        return name.to_owned();
    }
    let label = node.attribute("fmt").unwrap_or("[unknown]");
    // xctrace often formats processes as "name (PID)". Match names across recordings.
    if let Some((name, pid)) = label.rsplit_once(" (")
        && pid
            .strip_suffix(')')
            .is_some_and(|s| s.parse::<u32>().is_ok())
    {
        return name.to_owned();
    }
    label.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_refs_weights_leaf_order_and_missing_stacks() {
        let xml = include_str!("../tests/fixtures/before.xml");
        let p = parse(xml, "fixture".into(), None).unwrap();
        assert_eq!(p.samples, 4);
        assert_eq!(p.missing_stacks, 1);
        assert!((p.total - 40.0).abs() < f64::EPSILON);
        assert_eq!(p.stacks.len(), 2);
        let stack = p
            .stacks
            .keys()
            .find(|s| s.frames[0].symbol == "hot & busy")
            .unwrap();
        assert_eq!(stack.process, "demo");
        assert_eq!(stack.frames[1].symbol, "main");
        assert_eq!(stack.frames[0].image, "demo");
    }
    #[test]
    fn rejects_broken_references_and_invalid_weights() {
        let xml = include_str!("../tests/fixtures/before.xml");
        assert!(
            parse(
                &xml.replace("ref=\"1\"", "ref=\"999\""),
                String::new(),
                None
            )
            .is_err()
        );
        assert!(
            parse(
                &xml.replace(">10</weight>", ">NaN</weight>"),
                String::new(),
                None
            )
            .is_err()
        );
        assert!(
            parse(
                &xml.replace("name=\"time-profile\"", "name=\"allocations\""),
                String::new(),
                None
            )
            .is_err()
        );
    }
    #[test]
    fn cpu_profile_uses_cycle_weights_and_checks_empty_tables() {
        let xml = include_str!("../tests/fixtures/before.xml")
            .replace("time-profile", "cpu-profile")
            .replace("weight", "cycle-weight");
        let profile = parse(&xml, String::new(), None).unwrap();
        assert_eq!(profile.unit, "cycles");
        assert!((profile.total - 40.0).abs() < f64::EPSILON);
        assert!(
            parse(
                "<node><schema name='time-profile'/></node>",
                String::new(),
                None
            )
            .is_err()
        );
        assert!(
            parse(
                "<node><schema name='time-sample'/><row/></node>",
                String::new(),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn unweighted_exports_count_samples() {
        let xml = "<node><schema name='time-profile'/><row><backtrace id='1'><frame name='leaf'/></backtrace></row><row><backtrace ref='1'/></row></node>";
        let profile = parse(xml, String::new(), None).unwrap();
        assert_eq!(profile.unit, "samples");
        assert!((profile.total - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn supports_tagged_backtraces() {
        let xml =
            include_str!("../tests/fixtures/before.xml").replace("backtrace", "tagged-backtrace");
        assert_eq!(parse(&xml, String::new(), None).unwrap().samples, 4);
    }
    #[test]
    fn selects_latest_run_and_requested_schema() {
        let toc = "<trace-toc><run number='1'><data><table schema='time-profile'/></data></run><run number='2'><data><table schema='cpu-profile'/></data></run></trace-toc>";
        assert_eq!(
            select_table(
                toc,
                &Selection {
                    run: None,
                    schema: None
                }
            )
            .unwrap(),
            (2, "cpu-profile".into())
        );
        assert!(
            select_table(
                toc,
                &Selection {
                    run: Some(3),
                    schema: None
                }
            )
            .is_err()
        );
        assert!(
            select_table(
                toc,
                &Selection {
                    run: None,
                    schema: Some("time-profile".into())
                }
            )
            .is_err()
        );
    }
}
