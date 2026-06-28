//! Render a human-readable run report (also written to `report.md`).

pub fn render(
    project: &str,
    baseline: f64,
    best: f64,
    rounds: &[(u32, f64, bool)],
    held_out: usize,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("# skillsmith report — {project}\n\n"));
    s.push_str(&format!("- baseline gate score: {baseline:.3}\n"));
    s.push_str(&format!("- best gate score:     {best:.3}\n"));
    s.push_str(&format!("- lift:                {:+.3}\n", best - baseline));
    if held_out > 0 {
        s.push_str(&format!(
            "- gate: held-out validation ({held_out} val task(s))\n"
        ));
    }
    s.push('\n');
    s.push_str("## rounds\n");
    for (r, score, acc) in rounds {
        s.push_str(&format!(
            "- round {r}: {score:.3} {}\n",
            if *acc { "ACCEPTED" } else { "rejected" }
        ));
    }
    s.push('\n');
    if best > baseline {
        s.push_str("**Gate: improvement found — `skill.staged.md` is the improved skill.**\n");
    } else {
        s.push_str("**Gate: no measured improvement — staged skill equals the baseline best.**\n");
    }
    s
}
