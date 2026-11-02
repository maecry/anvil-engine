//! Plain-data description of a compiled graph (ADR-0012 §10) and its DOT / JSON renderings.

use std::fmt::Write as _;

use crate::plan::{Decls, Plan, ResKind, pass_io};

/// One pass of the frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDesc {
    /// Pass name (also its debug label).
    pub name: String,
    /// True when no root depends on it; it does not run.
    pub culled: bool,
    /// Names of textures it samples.
    pub reads: Vec<String>,
    /// Names of textures it renders to.
    pub writes: Vec<String>,
}

/// One texture of the frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    /// Texture name.
    pub name: String,
    /// True for imported (persistent) textures.
    pub imported: bool,
    /// Size in texels.
    pub extent: (u32, u32),
    /// Format; `None` for imports (not declared to the graph).
    pub format: Option<String>,
    /// First and last executed pass position using it; `None` when unused.
    pub lifetime: Option<(usize, usize)>,
    /// Physical slot shared by transients with disjoint lifetimes.
    pub slot: Option<usize>,
}

/// One planned barrier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BarrierDesc {
    /// Pass it precedes, `None` for the end of the frame (exports).
    pub before_pass: Option<String>,
    /// Texture name.
    pub texture: String,
    /// State before.
    pub from: String,
    /// State after.
    pub to: String,
}

/// The frame structure as plain data, passes in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphDescription {
    /// All declared passes, culled ones included.
    pub passes: Vec<PassDesc>,
    /// All declared textures.
    pub resources: Vec<ResourceDesc>,
    /// Barriers in execution order.
    pub barriers: Vec<BarrierDesc>,
}

pub(crate) fn describe(d: &Decls, p: &Plan) -> GraphDescription {
    let name = |t: crate::TexId| d.res[t.0 as usize].name.to_string();
    let passes = d
        .passes
        .iter()
        .enumerate()
        .map(|(i, pass)| {
            let (reads, writes) = pass_io(pass);
            PassDesc {
                name: pass.name.to_string(),
                culled: !p.live[i],
                reads: reads.into_iter().map(name).collect(),
                writes: writes.into_iter().map(name).collect(),
            }
        })
        .collect();
    let resources = d
        .res
        .iter()
        .enumerate()
        .map(|(i, r)| ResourceDesc {
            name: r.name.to_string(),
            imported: matches!(
                r.kind,
                ResKind::Imported { .. } | ResKind::ImportedBuf { .. }
            ),
            extent: r.extent(),
            format: match &r.kind {
                ResKind::Transient(t) => Some(format!("{:?}", t.format)),
                ResKind::TransientBuf(b) => Some(format!("buffer, {} bytes", b.size)),
                ResKind::Imported { .. } | ResKind::ImportedBuf { .. } => None,
            },
            lifetime: p.lifetime[i],
            slot: p.slot_of[i],
        })
        .collect();
    let barriers = p
        .barriers
        .iter()
        .map(|b| BarrierDesc {
            before_pass: p.order.get(b.before).map(|&i| d.passes[i].name.to_string()),
            texture: name(b.tex),
            from: b.from.name(),
            to: b.to.name(),
        })
        .collect();
    GraphDescription {
        passes,
        resources,
        barriers,
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl GraphDescription {
    /// Graphviz DOT: passes are boxes (culled ones dashed), textures ellipses (imports
    /// double), edges are reads and writes, labeled with the barrier planned before the pass.
    pub fn to_dot(&self) -> String {
        let mut s = String::from("digraph frame {\n  rankdir=LR;\n");
        for (i, p) in self.passes.iter().enumerate() {
            let style = if p.culled { "dashed" } else { "solid" };
            let _ = writeln!(
                s,
                "  p{i} [shape=box, style={style}, label=\"{}{}\"];",
                esc(&p.name),
                if p.culled { " (culled)" } else { "" }
            );
        }
        for (i, r) in self.resources.iter().enumerate() {
            let shape = if r.imported {
                "doubleoctagon"
            } else {
                "ellipse"
            };
            let _ = writeln!(
                s,
                "  r{i} [shape={shape}, label=\"{}\\n{}x{}\"];",
                esc(&r.name),
                r.extent.0,
                r.extent.1
            );
        }
        let res = |n: &str| self.resources.iter().position(|r| r.name == n).unwrap_or(0);
        let label = |pass: &str, tex: &str| {
            self.barriers
                .iter()
                .find(|b| b.before_pass.as_deref() == Some(pass) && b.texture == tex)
                .map_or(String::new(), |b| {
                    format!(" [label=\"{} -> {}\"]", b.from, b.to)
                })
        };
        for (i, p) in self.passes.iter().enumerate() {
            for t in &p.reads {
                let _ = writeln!(s, "  r{} -> p{i}{};", res(t), label(&p.name, t));
            }
            for t in &p.writes {
                let _ = writeln!(s, "  p{i} -> r{}{};", res(t), label(&p.name, t));
            }
        }
        for b in self.barriers.iter().filter(|b| b.before_pass.is_none()) {
            let _ = writeln!(
                s,
                "  r{} -> end [label=\"export {} -> {}\"];",
                res(&b.texture),
                b.from,
                b.to
            );
        }
        s.push_str("}\n");
        s
    }

    /// Compact JSON.
    pub fn to_json(&self) -> String {
        let list = |v: &[String]| {
            v.iter()
                .map(|x| format!("\"{}\"", esc(x)))
                .collect::<Vec<_>>()
                .join(",")
        };
        let passes = self
            .passes
            .iter()
            .map(|p| {
                format!(
                    "{{\"name\":\"{}\",\"culled\":{},\"reads\":[{}],\"writes\":[{}]}}",
                    esc(&p.name),
                    p.culled,
                    list(&p.reads),
                    list(&p.writes)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let opt = |o: Option<usize>| o.map_or("null".to_owned(), |v| v.to_string());
        let resources = self
            .resources
            .iter()
            .map(|r| {
                let life = r
                    .lifetime
                    .map_or("null".to_owned(), |(a, b)| format!("[{a},{b}]"));
                let fmt = r
                    .format
                    .as_ref()
                    .map_or("null".to_owned(), |f| format!("\"{f}\""));
                format!(
                    "{{\"name\":\"{}\",\"imported\":{},\"extent\":[{},{}],\"format\":{fmt},\"lifetime\":{life},\"slot\":{}}}",
                    esc(&r.name),
                    r.imported,
                    r.extent.0,
                    r.extent.1,
                    opt(r.slot)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let barriers = self
            .barriers
            .iter()
            .map(|b| {
                let before = b
                    .before_pass
                    .as_ref()
                    .map_or("null".to_owned(), |p| format!("\"{}\"", esc(p)));
                format!(
                    "{{\"before_pass\":{before},\"texture\":\"{}\",\"from\":\"{}\",\"to\":\"{}\"}}",
                    esc(&b.texture),
                    b.from,
                    b.to
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("{{\"passes\":[{passes}],\"resources\":[{resources}],\"barriers\":[{barriers}]}}")
    }
}
