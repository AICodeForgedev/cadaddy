//! The main workflows, written once: the MCP `workflows` tool returns [`to_json`] (whose `guide`
//! is [`guide_text`]) and `cadaddy-cli guide` prints the same text, so an agent and a person at a
//! shell read one guide.
//! `cadaddy-cli help` lists the workflows by name with their shell commands ([`summary_text`]).

/// One step of a workflow: what it does, the MCP call, and the same step from a shell.
pub struct Step {
    pub what: &'static str,
    /// The MCP tool call (`tool {params}`).
    pub mcp: &'static str,
    /// The `cadaddy-cli` command, copy-paste ready; empty when the step has no shell form.
    pub cli: &'static str,
}

pub struct Workflow {
    pub name: &'static str,
    pub description: &'static str,
    pub steps: &'static [Step],
}

pub const WORKFLOWS: &[Workflow] = &[
    Workflow {
        name: "pdf-to-bim",
        description: "PDF prints → BIM model → takeoff → estimate, step by step",
        steps: &[
            Step {
                what: "Build the model from a PDF or a folder of prints: import, check, building spec, stacked levels, saved as .cdy",
                mcp: "bim_from_pdf {path: \"prints\"} (cdy_paths says where the model was saved)",
                cli: "cadaddy-cli bim prints --out model.cdy",
            },
            Step {
                what: "Quantities by level (counts, lengths, areas with element handles), also written to a CSV",
                mcp: "execute {command: \"bim.takeoff\", params: {group_by: \"level\", file: \"takeoff.csv\"}}",
                cli: "cadaddy-cli run model.cdy --cmd 'bim.takeoff {\"group_by\":\"level\",\"file\":\"takeoff.csv\"}'",
            },
            Step {
                what: "Price the takeoff with a cost table (CSV/XLSX columns item, unit, material, labor_hrs)",
                mcp: "execute {command: \"bim.estimate\", params: {cost_table: \"costs.csv\", file: \"estimate.csv\"}}",
                cli: "cadaddy-cli run model.cdy --cmd 'bim.estimate {\"cost_table\":\"costs.csv\",\"file\":\"estimate.csv\"}'",
            },
        ],
    },
    Workflow {
        name: "guided-job",
        description: "The same pipeline run by CADaddy (import → classify → levels → rooms → devices → model → takeoff → estimate); it stops with questions when it needs an answer",
        steps: &[
            Step {
                what: "Start the job on a folder of prints and keep its state in a drawing",
                mcp: "guided_job {action: \"start\", params: {pdfs: \"prints\", goal: \"estimate\"}}",
                cli: "cadaddy-cli run --cmd 'job.start {\"pdfs\":\"prints\",\"goal\":\"estimate\"}' --save job.cdy",
            },
            Step { what: "The next question (or done)", mcp: "guided_job {action: \"next\"}", cli: "cadaddy-cli run job.cdy --cmd 'job.next {}'" },
            Step {
                what: "Answer it with the id from next; the pipeline goes on",
                mcp: "guided_job {action: \"answer\", params: {id: \"Q-classify-1\", answer: \"Correct\"}}",
                cli: "cadaddy-cli run job.cdy --cmd 'job.answer {\"id\":\"Q-classify-1\",\"answer\":\"Correct\"}' --save job.cdy",
            },
            Step {
                what: "Progress, then the final report",
                mcp: "guided_job {action: \"report\"}",
                cli: "cadaddy-cli run job.cdy --cmd 'job.status {}' --cmd 'job.report {}'",
            },
        ],
    },
    Workflow {
        name: "view-3d",
        description: "View a drawing in 3D with shading and lighting",
        steps: &[
            Step { what: "Open the model", mcp: "open {path: \"building.cdy\"}", cli: "" },
            Step { what: "SW isometric view", mcp: "execute {command: \"view.swiso\"}", cli: "" },
            Step { what: "Realistic shading", mcp: "execute {command: \"vscurrent\", params: {style: \"realistic\"}}", cli: "" },
            Step { what: "Render to PNG", mcp: "render {}", cli: "" },
        ],
    },
    Workflow {
        name: "plot-set",
        description: "Batch plot drawings to PDF",
        steps: &[Step {
            what: "One PDF per drawing in a folder",
            mcp: "plot {folder: \"drawings\", out: \"pdfs\"}",
            cli: "cadaddy-cli plot drawings pdfs",
        }],
    },
    Workflow {
        name: "bim-work",
        description: "Coordinate, take off and estimate a built BIM model: clashes with fixes, issues (BCF), quantities, cost",
        steps: &[
            Step { what: "Open a model built with bim_from_pdf or model.build", mcp: "open {path: \"model.cdy\"}", cli: "" },
            Step {
                what: "Clashes with ids, status and a viewpoint each",
                mcp: "execute {command: \"bim.clash\", params: {a: \"electrical\", b: \"mechanical\"}}",
                cli: "cadaddy-cli run model.cdy --cmd 'bim.clash {\"a\":\"electrical\",\"b\":\"mechanical\"}'",
            },
            Step {
                what: "Fix one: the lower-priority trade moves, then it is checked again",
                mcp: "execute {command: \"bim.clash.fix\", params: {id: \"C-1\", approve: true}}",
                cli: "",
            },
            Step {
                what: "Raise an issue, then export the issues as BCF",
                mcp: "execute {command: \"bim.issue.create\", params: {title: \"Duct over door\", kind: \"rfi\"}}, then bcf.export {path: \"issues.bcf\", version: \"3.0\"}",
                cli: "",
            },
            Step {
                what: "Quantities; bim.takeoff.show {line: 0} highlights one line in 3D",
                mcp: "execute {command: \"bim.takeoff\", params: {group_by: \"level\"}}",
                cli: "cadaddy-cli run model.cdy --cmd 'bim.takeoff {\"group_by\":\"level\"}'",
            },
            Step {
                what: "Cost, kept as revision A; compare_to: \"A\" later gives the change order",
                mcp: "execute {command: \"bim.estimate\", params: {cost_table: \"costs.csv\", revision: \"A\"}}",
                cli: "cadaddy-cli run model.cdy --cmd 'bim.estimate {\"cost_table\":\"costs.csv\",\"revision\":\"A\"}' --save model.cdy",
            },
        ],
    },
    Workflow {
        name: "convert-folder",
        description: "Convert a folder of drawings between formats (DWG, DXF, CDY, PDF)",
        steps: &[
            Step { what: "Every drawing in a folder", mcp: "", cli: "cadaddy-cli convert drawings converted --to cdy" },
            Step { what: "One file", mcp: "open {path: \"plan.dwg\"}, then save {path: \"plan.cdy\"}", cli: "cadaddy-cli convert plan.dwg plan.cdy" },
        ],
    },
];

const RECOVERY: &str = "If a call fails, its error message names the next call to make.";

/// The whole guide: every workflow with its steps, each step's MCP call and shell command.
pub fn guide_text() -> String {
    let mut out = String::from(
        "CADaddy workflows. Each step shows the MCP call (`mcp:`) and the same step from a shell (`cli:`).\n\
         Paths are examples: `prints` is a PDF or a folder of PDFs, `costs.csv` a cost table.\n",
    );
    for (n, w) in WORKFLOWS.iter().enumerate() {
        out.push_str(&format!("\n{}. {} — {}\n", n + 1, w.name, w.description));
        for (i, s) in w.steps.iter().enumerate() {
            out.push_str(&format!("   {}. {}\n", i + 1, s.what));
            if !s.mcp.is_empty() {
                out.push_str(&format!("      mcp: {}\n", s.mcp));
            }
            if !s.cli.is_empty() {
                out.push_str(&format!("      cli: {}\n", s.cli));
            }
        }
    }
    out.push('\n');
    out.push_str(RECOVERY);
    out.push('\n');
    out
}

/// The MCP `workflows` answer: each flow with its steps (`"N. what — call"`), the same steps as
/// shell commands (`cli`), an example call, and the whole [`guide_text`] as `guide`.
pub fn to_json() -> serde_json::Value {
    let flows: Vec<serde_json::Value> = WORKFLOWS
        .iter()
        .map(|w| {
            let steps: Vec<String> = w
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| if s.mcp.is_empty() { format!("{}. {}", i + 1, s.what) } else { format!("{}. {} — {}", i + 1, s.what, s.mcp) })
                .collect();
            let cli: Vec<&str> = w.steps.iter().map(|s| s.cli).filter(|c| !c.is_empty()).collect();
            let example = w.steps.iter().map(|s| s.mcp).find(|m| !m.is_empty()).unwrap_or_default();
            serde_json::json!({"name": w.name, "description": w.description, "steps": steps, "cli": cli, "example": example})
        })
        .collect();
    serde_json::json!({
        "workflows": flows,
        "error_recovery": RECOVERY,
        "quick_start": "PDF to BIM in one call: bim_from_pdf {path: \"your_plans.pdf\"}; from a shell: cadaddy-cli bim your_plans.pdf --out model.cdy",
        "guide": guide_text(),
    })
}

/// The workflows that run from a shell, by name with their commands (for `cadaddy-cli help`).
pub fn summary_text() -> String {
    let mut out = String::new();
    for w in WORKFLOWS {
        let cli: Vec<&str> = w.steps.iter().map(|s| s.cli).filter(|c| !c.is_empty()).collect();
        if cli.is_empty() {
            continue;
        }
        out.push_str(&format!("  {} — {}\n", w.name, w.description));
        for c in cli {
            out.push_str(&format!("    {c}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_workflow_goes_from_prints_to_an_estimate() {
        let first = WORKFLOWS.first().unwrap();
        let cli: Vec<&str> = first.steps.iter().map(|s| s.cli).collect();
        assert!(cli.first().unwrap().starts_with("cadaddy-cli bim "));
        assert!(cli.iter().any(|c| c.contains("bim.takeoff")));
        assert!(cli.last().unwrap().contains("bim.estimate"));
        assert!(summary_text().starts_with("  pdf-to-bim"));
    }

    #[test]
    fn every_step_names_a_real_command_and_valid_json() {
        for w in WORKFLOWS {
            assert!(!w.steps.is_empty(), "{}", w.name);
            for s in w.steps {
                assert!(!s.mcp.is_empty() || !s.cli.is_empty(), "{}: {}", w.name, s.what);
                // Each `--cmd 'id {json}'` names a command and parses as JSON.
                for part in s.cli.split("--cmd '").skip(1) {
                    let body = part.split('\'').next().unwrap();
                    let (id, json) = body.split_once(' ').unwrap();
                    assert!(crate::cmd::find_command(id).is_some(), "{}: no command {id}", w.name);
                    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|e| panic!("{}: {body}: {e}", w.name));
                    let spec = crate::cmd::find_command(id).unwrap();
                    assert!(crate::unknown_params(spec, &v).is_empty(), "{}: {body} has unknown params", w.name);
                }
                // `execute {command: "x"…}` names a command too.
                if let Some(rest) = s.mcp.strip_prefix("execute {command: \"") {
                    let id = rest.split('"').next().unwrap();
                    assert!(crate::cmd::find_command(id).is_some(), "{}: no command {id}", w.name);
                }
            }
        }
    }

    #[test]
    fn json_carries_the_guide_and_every_flow() {
        let v = to_json();
        assert_eq!(v["guide"], serde_json::json!(guide_text()));
        let flows = v["workflows"].as_array().unwrap();
        assert_eq!(flows.len(), WORKFLOWS.len());
        assert!(flows.iter().all(|f| f["steps"].as_array().is_some_and(|s| !s.is_empty())));
    }

    #[test]
    fn guide_has_every_step() {
        let g = guide_text();
        for w in WORKFLOWS {
            assert!(g.contains(w.name));
            for s in w.steps {
                assert!(g.contains(s.what) && g.contains(s.mcp) && g.contains(s.cli));
            }
        }
    }
}
