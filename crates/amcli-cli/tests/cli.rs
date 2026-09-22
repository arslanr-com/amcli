//! End-to-end tests over the real binary. These assert the contract an agent
//! depends on: exit codes it can branch on, records it can cut, and writes that
//! either land completely or not at all.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::prelude::*;

struct Model {
    dir: tempfile::TempDir,
}

impl Model {
    fn new(fixture: &str) -> Model {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus").join(fixture);
        std::fs::copy(&src, dir.path().join("m.archimate")).unwrap();
        Model { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("m.archimate")
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::cargo_bin("amcli")
            .unwrap()
            .arg("-m")
            .arg(self.path())
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }
}

fn rows(stdout: &str) -> Vec<Vec<&str>> {
    stdout.lines().filter(|l| !l.is_empty()).map(|l| l.split('\t').collect()).collect()
}

#[test]
fn records_go_to_stdout_and_context_goes_to_stderr() {
    let m = Model::new("modelimporter_test.archimate");
    let (code, out, err) = m.run(&["search", "BA"]);
    assert_eq!(code, 0);

    // stdout is nothing but records, so it pipes into `cut -f2` unchanged.
    for line in out.lines() {
        assert!(line.contains('\t'), "not a record: {line}");
    }
    assert!(!out.contains("total"), "counts belong on stderr, not in the data");
    let _ = err;
}

#[test]
fn exit_codes_distinguish_missing_from_ambiguous() {
    let m = Model::new("modelimporter_test.archimate");

    // A miss comes back with the nearest names, so the retry needs no second
    // exploratory search.
    let (code, _, err) = m.run(&["get", "BA111"]);
    assert_eq!(code, 3, "not found");
    assert!(err.contains("did you mean"), "{err}");
    assert!(err.contains("BA1"), "{err}");

    // Two concepts really sharing a name is a different answer with a different
    // remedy.
    m.run(&["element", "add", "BusinessActor", "Twin"]);
    m.run(&["element", "add", "BusinessRole", "Twin"]);
    let (code, _, err) = m.run(&["get", "Twin"]);
    assert_eq!(code, 4, "ambiguous");
    assert!(err.contains("2 concepts match"), "{err}");
    assert!(err.contains("id:"), "each candidate is a paste-ready selector: {err}");

    // And qualifying by type resolves it.
    let (code, _, _) = m.run(&["get", "BusinessActor:Twin"]);
    assert_eq!(code, 0);
}

#[test]
fn a_forbidden_relationship_is_refused_with_the_alternative_named() {
    let m = Model::new("modelimporter_test.archimate");
    m.run(&["element", "add", "DataObject", "Rec"]);
    m.run(&["element", "add", "ApplicationComponent", "Svc"]);

    let (code, _, err) = m.run(&["relation", "add", "Serving", "Rec", "Svc"]);
    assert_eq!(code, 5, "invalid");
    assert!(err.contains("does not permit Serving"), "{err}");
    assert!(err.contains("permitted here: Association"), "the error teaches: {err}");

    let (code, _, _) = m.run(&["relation", "add", "Association", "Rec", "Svc"]);
    assert_eq!(code, 0);
}

#[test]
fn an_edit_changes_only_the_lines_it_has_to() {
    let m = Model::new("modelimporter_test.archimate");
    let before = m.text();

    let (code, _, _) = m.run(&["element", "rename", "BA1", "Renamed Actor"]);
    assert_eq!(code, 0);

    let after = m.text();
    let differing = after.lines().zip(before.lines()).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 1, "renaming one element must not rewrite the file");
    assert_eq!(after, before.replace(r#"name="BA1""#, r#"name="Renamed Actor""#));
}

#[test]
fn deleting_refuses_until_told_and_then_leaves_no_dangling_reference() {
    let m = Model::new("testmodel1.archimate");
    let before = m.text();

    // The refusal IS the impact report, so the retry is informed.
    let (code, _, err) = m.run(&["element", "delete", "Business Actor"]);
    assert_eq!(code, 5);
    assert!(err.contains("also removes 5 other thing"), "{err}");
    assert!(err.contains("diagram_objects"), "{err}");
    assert_eq!(m.text(), before, "a refused delete writes nothing");

    // A dry run reports and still writes nothing.
    let (code, out, _) = m.run(&["element", "delete", "Business Actor", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(out.contains("true"), "dry_run is reported: {out}");
    assert_eq!(m.text(), before);

    let (code, _, _) = m.run(&["element", "delete", "Business Actor", "-y"]);
    assert_eq!(code, 0);

    let after = m.text();
    for gone in ["59fa6c90", "ffdc8ea9", "eac5adf1", "f408e9d0"] {
        assert!(!after.contains(gone), "{gone} survived");
    }
    assert!(!after.contains("targetConnections"), "the derived mirror was recomputed");

    let (code, _, err) = m.run(&["validate", "--level", "integrity"]);
    assert_eq!(code, 0, "the model still loads and still checks out: {err}");
}

#[test]
fn a_stale_checksum_refuses_the_write() {
    let m = Model::new("modelimporter_test.archimate");
    let before = m.text();

    let (code, _, err) = m.run(&["element", "rename", "BA1", "X", "--expect-checksum", "deadbeef"]);
    assert_eq!(code, 6, "conflict");
    assert!(err.contains("changed since"), "{err}");
    assert_eq!(m.text(), before, "nothing was applied");

    // With the real checksum it goes through.
    let (_, out, _) = m.run(&["info", "-F", "json", "-q"]);
    let checksum = out.split(r#""checksum":""#).nth(1).unwrap().split('"').next().unwrap();
    let (code, _, _) = m.run(&["element", "rename", "BA1", "X", "--expect-checksum", checksum]);
    assert_eq!(code, 0);
}

#[test]
fn trace_returns_nodes_and_edges_as_flat_records() {
    let m = Model::new("modelimporter_test.archimate");
    let (code, out, _) = m.run(&["trace", "BA1", "-n", "2"]);
    assert_eq!(code, 0);

    let r = rows(&out);
    assert!(r.iter().any(|row| row[0] == "node"));
    assert!(r.iter().any(|row| row[0] == "edge"), "edges are records, not a count: {out}");
    // Edges are keyed by id, because two concepts can share a name.
    let edge = r.iter().find(|row| row[0] == "edge").unwrap();
    assert!(edge[1].len() > 8, "an edge carries its own id: {edge:?}");
}

#[test]
fn token_economy_flags_do_what_they_say() {
    let m = Model::new("modelimporter_test.archimate");

    // --count answers "how many" without paying for the rows.
    let (code, out, _) = m.run(&["list", "--count"]);
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1);
    assert!(out.trim().parse::<usize>().is_ok(), "{out}");

    // --fields projects.
    let (_, out, _) = m.run(&["list", "--fields", "id,name"]);
    for row in rows(&out) {
        assert_eq!(row.len(), 2, "{row:?}");
    }

    // Subtractive projection drops instead of keeping.
    let (_, full, _) = m.run(&["list"]);
    let (_, less, _) = m.run(&["list", "--fields", "-folder"]);
    assert_eq!(rows(&less)[0].len(), rows(&full)[0].len() - 1);

    // -q quietens stderr and leaves stdout alone, in every format — so the
    // JSON envelope is there either way and one jq path reads both.
    let (_, plain, _) = m.run(&["list", "-F", "json"]);
    let (_, quiet, err) = m.run(&["list", "-F", "json", "-q"]);
    assert_eq!(plain, quiet, "-q must not reshape stdout");
    assert!(quiet.trim_start().starts_with(r#"{"ok":true,"data":["#), "{quiet}");
    assert!(err.is_empty(), "-q asked for no commentary: {err}");
}

#[test]
fn json_output_is_valid_and_carries_the_envelope() {
    let m = Model::new("modelimporter_test.archimate");
    let (_, out, _) = m.run(&["get", "BA1", "-F", "json"]);
    assert!(out.contains(r#""ok":true"#));
    assert!(out.contains(r#""data":["#));
    assert!(out.contains(r#""meta":{"#));
    // Relationship ids are present, which is the only way to address one.
    assert!(out.contains(r#""relations":[{"id":"#), "{out}");

    let (_, out, _) = m.run(&["get", "nope", "-F", "json"]);
    assert!(out.contains(r#""ok":false"#));
    assert!(out.contains(r#""exit":3"#), "the exit code is in the payload too: {out}");
}

/// Reported from real use: `get` on a relationship answered with an empty
/// `relations` list — nothing points at it, which is true and useless — and
/// `query 'kind=relation'` gave a type with nothing to hang it on. Checking
/// what a relationship joined took a second command against one of its ends.
#[test]
fn a_relationship_row_says_what_it_joins() {
    let m = Model::new("modelimporter_test.archimate");

    let (code, out, _) =
        m.run(&["query", "kind=relation", "--fields", "id,source_name,target_name", "-q"]);
    assert_eq!(code, 0);
    let r = rows(&out);
    assert!(!r.is_empty());
    for row in &r {
        assert_eq!(row.len(), 3, "{row:?}");
        assert!(!row[1].is_empty() && !row[2].is_empty(), "both ends are named: {row:?}");
    }

    // And on the relationship itself, with the ids that address each end.
    let rel = r[0][0];
    let (code, out, _) = m.run(&["get", &format!("id:{rel}"), "-F", "json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let row = &v["data"][0];
    assert_eq!(row["source_name"], "BA1", "{out}");
    assert_eq!(row["target_name"], "BR1", "{out}");
    assert!(row["source"].as_str().unwrap().len() > 8, "an end is addressable: {out}");

    // An element carries no ends, rather than two empty columns. Its nested
    // relations do — that is a different record shape, checked below.
    let (_, out, _) = m.run(&["get", "BA1", "-F", "json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["data"][0].get("source_name").is_none(), "{out}");

    // `get` names the views once. It used to say `views` twice in one object —
    // the count and then the list — and a JSON reader keeps whichever it saw
    // last.
    assert_eq!(out.matches(r#""views":"#).count(), 1, "{out}");
}

#[test]
fn a_bad_filter_says_what_the_fields_are() {
    let m = Model::new("modelimporter_test.archimate");
    let (code, _, err) = m.run(&["query", "bogus=1"]);
    assert_eq!(code, 2, "usage");
    assert!(err.contains("unknown field"), "{err}");
    assert!(err.contains("layer"), "{err}");
}

#[test]
fn validate_reports_findings_on_stdout_and_the_verdict_in_the_exit_code() {
    let m = Model::new("testDeleteHandler.archimate");
    let (code, out, _) = m.run(&["validate", "--level", "rules"]);
    assert_eq!(code, 5, "the fixture carries two matrix violations");

    let r = rows(&out);
    assert!(r.iter().any(|row| row[0] == "REL2001"));
    // Every finding names a line and a fix.
    for row in r.iter().filter(|row| row[0] == "REL2001") {
        assert!(row[4].parse::<u32>().unwrap() > 0, "line: {row:?}");
        assert!(row.last().unwrap().starts_with("amcli "), "runnable fix: {row:?}");
    }

    // Levels are cumulative, so integrity still reports them.
    let (code, _, _) = m.run(&["validate", "--level", "integrity"]);
    assert_eq!(code, 5);

    // Stopping at types says nothing about them: these are legality problems,
    // not schema ones.
    let (code, out, _) = m.run(&["validate", "--level", "types"]);
    assert_eq!(code, 0);
    assert!(!out.contains("REL2001"));
}

#[test]
fn model_discovery_walks_up_and_refuses_to_guess() {
    let m = Model::new("modelimporter_test.archimate");
    let nested = m.dir.path().join("a/b");
    std::fs::create_dir_all(&nested).unwrap();

    let out =
        Command::cargo_bin("amcli").unwrap().current_dir(&nested).arg("info").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "the model one directory up is found");

    // Two models in the same directory is ambiguous, not a coin toss.
    std::fs::copy(m.path(), m.dir.path().join("other.archimate")).unwrap();
    let out = Command::cargo_bin("amcli")
        .unwrap()
        .current_dir(m.dir.path())
        .arg("info")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("pass -m"));
}

#[test]
fn every_write_leaves_a_model_that_still_loads() {
    let m = Model::new("modelimporter_test.archimate");
    let steps: &[&[&str]] = &[
        &["element", "add", "ApplicationComponent", "Svc", "--doc", "Docs & more"],
        &["element", "add", "DataObject", "Rec"],
        &["relation", "add", "Access", "Svc", "Rec", "--access", "rw"],
        &["prop", "set", "Svc", "owner", "team-a"],
        &["folder", "add", "/Application", "Payments"],
        &["element", "move", "Svc", "-f", "/Application/Payments"],
        &["element", "rename", "Svc", "Renamed"],
    ];
    for s in steps {
        let (code, _, err) = m.run(s);
        assert_eq!(code, 0, "{s:?} failed: {err}");
    }

    let (code, out, _) = m.run(&["get", "Renamed", "-F", "json"]);
    assert_eq!(code, 0);
    assert!(out.contains(r#""folder":"/Application/Payments""#), "{out}");
    assert!(out.contains(r#""key":"owner""#), "{out}");
    // The documentation was escaped on the way in and comes back intact.
    assert!(out.contains("Docs & more"), "{out}");

    let (code, _, err) = m.run(&["validate"]);
    assert_eq!(code, 0, "{err}");
}

/// A view built member by member is a dozen commands, each writing the file
/// and any one able to fail halfway. In a batch the view ops land with the
/// concept edits they belong to, once, and `--dry-run` covers them too.
#[test]
fn a_batch_can_build_and_lay_out_a_view() {
    let m = Model::new("modelimporter_test.archimate");
    let ops = m.dir.path().join("view.jsonl");
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"element.add","type":"ApplicationComponent","name":"Refund Service","ref":"r","if_absent":true}"#,
            "\n",
            r#"{"op":"element.add","type":"DataObject","name":"Refund Record","ref":"rec","if_absent":true}"#,
            "\n",
            r#"{"op":"relation.add","type":"Access","source":"ref:r","target":"ref:rec","access":"rw","if_absent":true}"#,
            "\n",
            r#"{"op":"view.create","name":"Refunds","replace":true}"#,
            "\n",
            // A ref, a plain name, and something already in the model.
            r#"{"op":"view.add","view":"Refunds","target":"ref:r"}"#,
            "\n",
            r#"{"op":"view.add","view":"Refunds","target":"Refund Record"}"#,
            "\n",
            r#"{"op":"view.add","view":"Refunds","target":"BA1"}"#,
            "\n",
            r#"{"op":"view.layout","view":"Refunds","relayout_all":true}"#,
            "\n",
            r#"{"op":"view.auto","name":"Around Refunds","from":"ref:r","depth":1,"replace":true}"#,
            "\n",
            r#"{"op":"view.rename","view":"Around Refunds","name":"Refund Neighbourhood"}"#,
            "\n",
        ),
    )
    .unwrap();

    // Dry run: every line reports, nothing is written.
    let before = m.text();
    let (code, out, _) = m.run(&["apply", ops.to_str().unwrap(), "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(rows(&out).len(), 10);
    assert_eq!(m.text(), before, "a dry run writes nothing");
    assert!(
        !out.contains("dry_run"),
        "the view rows do not each claim dry-run; the batch says so once: {out}"
    );

    // For real: the view exists with three objects, the access relationship
    // drawn between two of them, and the second view renamed.
    let (code, out, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("view.create") && out.contains("view.layout"), "{out}");
    let (_, listing, _) = m.run(&["view", "list", "-q"]);
    assert!(listing.contains("Refunds"), "{listing}");
    assert!(listing.contains("Refund Neighbourhood"), "{listing}");
    assert!(!listing.contains("Around Refunds"), "renamed, not duplicated: {listing}");
    let (_, json, _) = m.run(&["view", "render", "Refunds", "--as", "json"]);
    let scene: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(scene["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(scene["edges"].as_array().unwrap().len(), 1, "the access edge is drawn");

    // Seeded, a re-run is a no-op byte for byte — the property the whole
    // batch design exists for, and view ops must not break it. The rename is
    // left out of this batch: a rename cannot be re-run, and the batch says
    // so like a second `view rename` at the prompt would.
    let again = m.dir.path().join("again.jsonl");
    let text = std::fs::read_to_string(&ops).unwrap();
    let text: String =
        text.lines().filter(|l| !l.contains("view.rename")).map(|l| format!("{l}\n")).collect();
    std::fs::write(&again, text).unwrap();
    let seeded = |m: &Model| {
        Command::cargo_bin("amcli")
            .unwrap()
            .env("AMCLI_ID_SEED", "t")
            .arg("-m")
            .arg(m.path())
            .args(["apply", again.to_str().unwrap()])
            .output()
            .unwrap()
    };
    let r = seeded(&m);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let first = m.text();
    assert!(seeded(&m).status.success());
    assert_eq!(m.text(), first, "a seeded re-run changes nothing");

    // A bad view line abandons the batch like any other.
    let bad = m.dir.path().join("bad.jsonl");
    std::fs::write(
        &bad,
        concat!(
            r#"{"op":"element.add","type":"Goal","name":"Would Be Added"}"#,
            "\n",
            r#"{"op":"view.add","view":"Refunds","target":"No Such Thing"}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", bad.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(err.contains("line 2"), "{err}");
    assert_eq!(m.text(), first, "nothing from the failed batch was written");
}

#[test]
fn a_batch_lands_completely_or_not_at_all() {
    let m = Model::new("modelimporter_test.archimate");
    let ops = m.dir.path().join("ops.jsonl");

    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"element.add","type":"ApplicationComponent","name":"Refund Service","ref":"r","if_absent":true}"#,
            "\n",
            r#"{"op":"element.add","type":"DataObject","name":"Refund Record","ref":"rec","if_absent":true}"#,
            "\n",
            r#"{"op":"relation.add","type":"Access","source":"ref:r","target":"ref:rec","access":"rw","if_absent":true}"#,
            "\n",
        ),
    )
    .unwrap();

    let (code, out, _) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(rows(&out).len(), 3);

    // `if_absent` makes the whole batch re-runnable, byte for byte.
    let after_first = m.text();
    let (code, _, _) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(m.text(), after_first, "a re-run changes nothing");

    // One bad line and the file is untouched — there is no partial state to
    // clean up, because the write only happens once at the end.
    let bad = m.dir.path().join("bad.jsonl");
    std::fs::write(
        &bad,
        concat!(
            r#"{"op":"element.add","type":"ApplicationComponent","name":"Would Be Added"}"#,
            "\n",
            r#"{"op":"relation.add","type":"Serving","source":"Refund Record","target":"Refund Service"}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", bad.to_str().unwrap()]);
    assert_eq!(code, 5);
    assert!(err.contains("line 2"), "the failing line is named: {err}");
    assert_eq!(m.text(), after_first, "nothing from the failed batch was written");
    assert!(!m.text().contains("Would Be Added"), "not even the line that succeeded");
}

/// Reported from real use: replacing an Association with a Realization needed
/// a delete and an add, and the batch could only do the add — so the model
/// passed through a state where it said something false, or the delete was
/// left to a second command that could fail on its own.
#[test]
fn a_batch_replaces_a_relationship_in_one_write() {
    let m = Model::new("modelimporter_test.archimate");
    m.run(&["element", "add", "ApplicationComponent", "Payment API"]);
    m.run(&["element", "add", "ApplicationService", "Payments"]);
    m.run(&["relation", "add", "Association", "Payment API", "Payments"]);
    m.run(&["prop", "set", "Payment API", "owner", "team-a"]);

    let (_, out, _) = m.run(&["query", "type=Association", "--fields", "id", "-q"]);
    let old = out.trim().to_string();

    let ops = m.dir.path().join("swap.jsonl");
    std::fs::write(
        &ops,
        format!(
            concat!(
                r#"{{"op":"relation.delete","target":"id:{id}","if_present":true}}"#,
                "\n",
                r#"{{"op":"relation.add","type":"Realization","source":"Payment API","target":"Payments","if_absent":true}}"#,
                "\n",
                r#"{{"op":"prop.unset","target":"Payment API","key":"owner"}}"#,
                "\n",
            ),
            id = old
        ),
    )
    .unwrap();

    let (code, out, _) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(!m.text().contains("AssociationRelationship"), "the old one is gone");
    assert!(m.text().contains("RealizationRelationship"), "the new one is there");
    assert!(!m.text().contains(r#"key="owner""#), "prop.unset removed it");

    // And the whole thing is re-runnable: nothing to delete, nothing to add,
    // nothing to unset, so the file comes back byte-identical.
    let after = m.text();
    let (code, out, _) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(m.text(), after, "a re-run changes nothing");
    let skipped = rows(&out).into_iter().find(|r| r[0] == "relation.delete").unwrap();
    assert_eq!(skipped[1], "", "a skipped delete reports no id: {skipped:?}");
    assert_eq!(skipped[2], "0", "and removes nothing: {skipped:?}");

    // Without `if_present` the miss is the batch's problem, not a silent skip.
    let strict = m.dir.path().join("strict.jsonl");
    std::fs::write(&strict, format!("{{\"op\":\"relation.delete\",\"target\":\"id:{old}\"}}\n"))
        .unwrap();
    let (code, _, err) = m.run(&["apply", strict.to_str().unwrap()]);
    assert_eq!(code, 3, "not found");
    assert!(err.contains("line 1"), "{err}");

    // And it refuses an element: aimed at one by accident it would take the
    // element's whole cascade with it.
    let wrong = m.dir.path().join("wrong.jsonl");
    std::fs::write(&wrong, "{\"op\":\"relation.delete\",\"target\":\"Payment API\"}\n").unwrap();
    let (code, _, err) = m.run(&["apply", wrong.to_str().unwrap()]);
    assert_eq!(code, 2, "usage");
    assert!(err.contains("is not a relationship"), "{err}");
    assert!(err.contains("element.delete"), "the error names the op that would work: {err}");
    assert_eq!(m.text(), after, "and nothing was written");
}

/// `references/batch.md` is what an agent reads before writing a batch — not
/// `--help`, which says nothing about the operations. A field that exists in
/// the parser and nowhere in that file is a feature nobody can use:
/// `relation.add` accepted a `doc` for two releases without saying so.
#[test]
fn every_batch_op_and_field_is_documented() {
    const SRC: &str = include_str!("../src/apply.rs");
    const DOC: &str = include_str!("../../../skills/amcli/references/batch.md");

    let body = SRC.split("enum Op {").nth(1).expect("the Op enum").split("\n}\n").next().unwrap();

    // `op` is every documented line for the operation being read, `shown` its
    // name for the complaint.
    let mut op = String::new();
    let mut shown = String::new();
    let mut renamed: Option<String> = None;
    let mut missing: Vec<String> = Vec::new();
    let mut seen = 0;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#[serde(rename = \"") {
            let name = rest.split('"').next().unwrap().to_string();
            // An operation is named with a dot, a field never is.
            if name.contains('.') {
                // Every field has to appear on a line that documents *this*
                // operation. Checking the file as a whole would have passed
                // the case that prompted the test: `relation.add` took a `doc`
                // and only `element.add` was shown taking one.
                op = DOC
                    .lines()
                    .filter(|l| l.contains(&format!(r#""op":"{name}""#)))
                    .collect::<Vec<_>>()
                    .join(" ");
                if op.is_empty() {
                    missing.push(format!("op {name}"));
                }
                shown = name;
                seen += 1;
            } else {
                renamed = Some(name);
            }
            continue;
        }
        if line.starts_with('#') || line.starts_with("//") || line.is_empty() {
            continue;
        }
        for field in fields_of(line) {
            let field = renamed.take().unwrap_or(field);
            if !op.contains(&format!(r#""{field}":"#)) {
                missing.push(format!("{shown}.{field}"));
            }
        }
    }
    // A parser that stopped reading the enum early would pass by finding
    // nothing to complain about.
    assert!(seen > 15, "only {seen} operations parsed out of `Op`");
    assert!(missing.is_empty(), "not in references/batch.md: {missing:?}");
}

/// The `name: Type` pairs in one line of the `Op` enum, which is all the
/// parsing the test above needs.
fn fields_of(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        if b[i] != b':' || b.get(i + 1) != Some(&b' ') || (i > 0 && b[i - 1] == b':') {
            continue;
        }
        let start =
            line[..i].rfind(|c: char| !c.is_alphanumeric() && c != '_').map(|p| p + 1).unwrap_or(0);
        if start < i {
            out.push(line[start..i].to_string());
        }
    }
    out
}

#[test]
fn a_ref_must_be_defined_before_it_is_used() {
    let m = Model::new("modelimporter_test.archimate");
    let ops = m.dir.path().join("ops.jsonl");
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"relation.add","type":"Serving","source":"ref:later","target":"BA1"}"#,
            "\n",
            r#"{"op":"element.add","type":"ApplicationComponent","name":"Later","ref":"later"}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 3);
    assert!(err.contains("no earlier line named `later`"), "{err}");
}

#[test]
fn views_can_be_generated_and_drawn() {
    let m = Model::new("modelimporter_test.archimate");
    let (code, out, _) =
        m.run(&["view", "auto", "Generated", "--from", "BA1", "-n", "2", "--layout", "layered"]);
    assert_eq!(code, 0, "{out}");

    let svg = m.dir.path().join("v.svg");
    let (code, _, _) = m.run(&["view", "render", "Generated", "-o", svg.to_str().unwrap()]);
    assert_eq!(code, 0);

    let body = std::fs::read_to_string(&svg).unwrap();
    assert!(body.starts_with("<svg xmlns="));
    assert!(body.contains("BA1"));
    // Edges after nodes, matching GEF's layer order.
    assert!(body.find("class=\"nodes\"") < body.find("class=\"edges\""));

    // A generated view is a valid model, not just a picture.
    let (code, _, err) = m.run(&["validate", "--level", "integrity"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn rendering_an_existing_view_keeps_the_geometry_the_file_records() {
    let m = Model::new("testmodel1.archimate");
    let (code, out, _) = m.run(&["view", "render", "2 Test Bounds and Images", "--as", "json"]);
    assert_eq!(code, 0);

    // The actor sits inside a group at (156,204) with a relative (36,42).
    assert!(out.contains(r#""x":192,"y":246"#), "nested coordinates were summed: {out}");
    // The Business layer fill, and nothing invented.
    assert!(out.contains("\"fill\":\"#ffffb5\""), "{out}");
}

#[test]
fn exports_say_what_they_are() {
    let m = Model::new("modelimporter_test.archimate");

    let (code, out, _) = m.run(&["export", "mermaid"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("%% Generated by amcli"));
    // A format that re-lays-out has to say so, or it gets mistaken for the
    // diagram someone drew.
    assert!(out.contains("re-lays-out"), "{out}");
    assert!(out.contains("flowchart TD"));

    let (code, out, _) = m.run(&["export", "csv"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("id,type,name,layer,folder,source,target,documentation\n"));

    let (code, _, err) = m.run(&["export", "pdf"]);
    assert_eq!(code, 8, "unsupported");
    assert!(err.contains("view render"), "the faithful path is named: {err}");
}

#[test]
fn the_skill_installs_where_agents_look_and_uninstalls_cleanly() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude/skills")).unwrap();

    let run = |args: &[&str]| {
        Command::cargo_bin("amcli").unwrap().env("HOME", home.path()).args(args).output().unwrap()
    };

    let out = run(&["skill", "install"]);
    assert_eq!(out.status.code(), Some(0));

    let skill = home.path().join(".agents/skills/amcli");
    assert!(skill.join("SKILL.md").exists(), "the documented cross-tool location");
    assert!(skill.join("references/types.md").exists());
    // The skill is what teaches an agent to install the binary, so the
    // installer has to travel with it rather than be fetched from a URL.
    assert!(skill.join("scripts/install.sh").exists());

    // Nothing is generated into the directory: `npx skills add` copies
    // `skills/amcli/` verbatim, and anything written only by this command
    // would make the two routes disagree.
    assert!(
        !skill.join("references/commands.md").exists(),
        "the command reference is a command, not a file"
    );

    // One link for Claude Code; Codex reads ~/.agents/skills natively.
    let link = home.path().join(".claude/skills/amcli");
    #[cfg(unix)]
    assert_eq!(std::fs::read_link(&link).unwrap(), skill);
    // Windows needs a privilege for symlinks that a normal user does not have,
    // so there it is a copy and only the content can be compared.
    #[cfg(not(unix))]
    assert_eq!(
        std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap()
    );

    // The frontmatter carries only fields the Agent Skills spec defines, or
    // strict validators reject the file.
    let body = std::fs::read_to_string(skill.join("SKILL.md")).unwrap();
    let front = body.split("---").nth(1).unwrap();
    for line in front.lines().filter(|l| !l.starts_with(' ') && l.contains(':')) {
        let key = line.split(':').next().unwrap().trim();
        assert!(
            ["name", "description", "license", "compatibility", "metadata"].contains(&key),
            "`{key}` is not an Agent Skills field"
        );
    }

    assert_eq!(run(&["skill", "install"]).status.code(), Some(0), "installing twice is fine");
    assert_eq!(run(&["skill", "uninstall"]).status.code(), Some(0));
    assert!(!skill.exists());
    assert!(std::fs::read_link(&link).is_err());
}

/// `npx skills add` copies `skills/amcli/` out of the repository; this binary
/// writes the copy compiled into it. If those two ever differ, an agent gets
/// different instructions depending on how it installed, and the conflict
/// check in `skill install` starts firing on content it wrote itself.
///
/// Adding a file to `skills/amcli/` without adding it to `FILES` is the way
/// that happens, so this walks the directory rather than the list.
#[test]
fn both_install_routes_ship_the_same_bytes() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/amcli");
    let home = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("amcli")
        .unwrap()
        .env("HOME", home.path())
        .args(["skill", "install"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let installed = home.path().join(".agents/skills/amcli");

    let mut checked = 0;
    let mut stack = vec![source.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(&source).unwrap();
            let want = std::fs::read(&path).unwrap();
            let got = std::fs::read(installed.join(rel)).unwrap_or_else(|_| {
                panic!("{} is in skills/amcli but not embedded in the binary", rel.display())
            });
            assert!(want == got, "{} differs between the two install routes", rel.display());
            checked += 1;
        }
    }
    assert!(checked >= 5, "expected the whole skill, walked only {checked} files");
}

/// The command reference is a command, so it cannot describe a release other
/// than the one running.
#[test]
fn the_command_reference_comes_from_the_binary() {
    let out = Command::cargo_bin("amcli").unwrap().args(["skill", "commands"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("--expect-checksum"));
    assert!(text.contains("amcli element"));
    assert!(text.contains("amcli skill"));
}

/// Two things in SKILL.md that an agent executes literally, so a typo in
/// either is a broken recovery path rather than a documentation nit.
#[test]
fn the_skill_points_at_paths_that_exist_and_never_downgrades_itself() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/amcli");
    let body = std::fs::read_to_string(source.join("SKILL.md")).unwrap();

    // Under `npx skills add` the skill ships from the default branch and the
    // binary from the newest tag, so the skill is the *newer* of the two. An
    // instruction to run `skill install --force` on a missing command would
    // overwrite it with the older binary's copy and strand the npx lock file.
    assert!(
        !body.contains("skill install --force"),
        "that instruction downgrades the skill when the binary is the stale one"
    );

    // Both spellings: the PowerShell line uses backslashes, and a typo there
    // is just as broken as one in the sh line.
    let mut found = 0;
    for word in body.split_whitespace() {
        let word = word.replace('\\', "/");
        let Some(rest) = word.strip_prefix("~/.agents/skills/amcli/") else { continue };
        let rel = rest.trim_end_matches(['`', '"', ')', ',', '.']);
        assert!(
            source.join(rel).exists(),
            "SKILL.md tells the agent to run {rel}, which is not in the skill"
        );
        found += 1;
    }
    assert!(found >= 2, "expected the sh and PowerShell installers to be named, saw {found}");

    // The skill names the amcli version it is written for. It ships from the
    // branch, so that number has to move with every release commit, and this
    // is what makes forgetting it a red test rather than a stale document.
    let stated = body
        .lines()
        .find_map(|l| {
            let (_, rest) = l.split_once("written for **amcli ")?;
            rest.split_once("**").map(|(v, _)| v.trim().to_string())
        })
        .expect("SKILL.md says which amcli it is written for");
    assert_eq!(
        stated,
        env!("CARGO_PKG_VERSION"),
        "SKILL.md says it is written for amcli {stated}, but this is {}; bump the skill with the release",
        env!("CARGO_PKG_VERSION")
    );
}

/// A skill newer than the binary is the expected steady state, so the failure
/// has to say so where the agent is already reading.
#[test]
fn an_unknown_subcommand_blames_the_binary_not_the_skill() {
    let out = Command::cargo_bin("amcli").unwrap().arg("frobnicate").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "usage");
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("older"), "names the cause: {err}");
    assert!(err.contains("scripts/install.sh"), "gives a runnable recovery: {err}");
}

/// `--count` is documented as printing how many results there would be and
/// nothing else. On `view auto` it also created the view, so the command
/// documented as the safe way to ask a question was the one that left duplicate
/// views behind.
#[test]
fn count_answers_the_question_without_writing() {
    let m = Model::new("testmodel1.archimate");
    let before = m.text();

    let (code, out, _) = m.run(&["view", "auto", "probe", "--from", "Business Actor", "--count"]);
    assert_eq!(code, 0);
    assert!(out.trim().parse::<usize>().is_ok(), "a count and nothing else: {out}");
    assert_eq!(m.text(), before, "--count wrote to the model");
    assert!(!m.text().contains("probe"), "the view was created anyway");

    // Every other write path answers it the same way.
    for args in [
        &["element", "add", "BusinessActor", "Counted", "--count"][..],
        &["view", "create", "Counted", "--count"][..],
        &["element", "delete", "Business Actor", "-y", "--count"][..],
    ] {
        let (code, _, _) = m.run(args);
        assert_eq!(code, 0, "{args:?}");
        assert_eq!(m.text(), before, "{args:?} wrote to the model");
    }
}

/// Two views with the same name are indistinguishable to every selector, and
/// there used to be no way to remove either one.
#[test]
fn a_view_name_cannot_be_taken_twice_and_can_be_given_back() {
    let m = Model::new("testmodel1.archimate");
    assert_eq!(m.run(&["view", "create", "Flow"]).0, 0);

    let (code, _, err) = m.run(&["view", "create", "Flow"]);
    assert_eq!(code, 6, "conflict");
    assert!(err.contains("already called `Flow`"), "{err}");
    assert!(err.contains("--replace"), "the way forward is named: {err}");

    // `view auto` is the one that actually bit, and it answers the same way.
    let (code, _, _) = m.run(&["view", "auto", "Flow", "--from", "Business Actor"]);
    assert_eq!(code, 6);
    let (code, _, _) = m.run(&["view", "auto", "Flow", "--from", "Business Actor", "--replace"]);
    assert_eq!(code, 0);
    assert_eq!(named_views(&m, "Flow"), 1, "--replace replaced rather than added");

    // Renaming refuses the same clash, and then works.
    assert_eq!(m.run(&["view", "create", "Other"]).0, 0);
    assert_eq!(m.run(&["view", "rename", "Other", "Flow"]).0, 6);
    assert_eq!(m.run(&["view", "rename", "Other", "Renamed"]).0, 0);
    assert_eq!(named_views(&m, "Renamed"), 1);

    // And a stray view can be removed, which is what forced whole-model rebuilds.
    assert_eq!(m.run(&["view", "delete", "Renamed"]).0, 0);
    assert_eq!(named_views(&m, "Renamed"), 0);
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

fn named_views(m: &Model, name: &str) -> usize {
    let (_, out, _) = m.run(&["view", "list", "-q"]);
    rows(&out).iter().filter(|r| r.get(1) == Some(&name)).count()
}

/// Deleting a view drawn as a reference box on another view has to take the box
/// with it: `model="…"` pointing at nothing is a file Archi will not open.
#[test]
fn deleting_a_referenced_view_refuses_until_told_and_leaves_nothing_dangling() {
    let m = Model::new("testDeleteHandler.archimate");
    let before = m.text();

    let (code, _, err) = m.run(&["view", "delete", "id:12917bec"]);
    assert_eq!(code, 5);
    assert!(err.contains("drawn as a reference"), "{err}");
    assert_eq!(m.text(), before, "a refused delete writes nothing");

    let (code, out, _) = m.run(&["view", "delete", "id:12917bec", "-y"]);
    assert_eq!(code, 0, "{out}");
    assert!(!m.text().contains("12917bec"), "the view survived");
    assert!(!m.text().contains("99a52921"), "the reference box now dangles");

    // The fixture carries two matrix violations of its own, so integrity is
    // compared against the baseline rather than to zero.
    let (_, out, _) = m.run(&["validate", "--level", "integrity", "-q"]);
    assert!(!out.contains("99a52921"), "a dangling visual was reported: {out}");
}

/// An added concept used to stay a floating box even when the thing it relates
/// to was already on the same view, and no amount of re-laying-out could fix
/// that because the connection was never written.
#[test]
fn adding_a_concept_to_a_view_draws_the_relationships_it_brings() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["element", "add", "ApplicationComponent", "Svc"]).0, 0);
    assert_eq!(m.run(&["relation", "add", "Serving", "Svc", "BA1"]).0, 0);
    assert_eq!(m.run(&["view", "create", "Wired"]).0, 0);

    let edges = |m: &Model| {
        let (_, out, _) = m.run(&["view", "render", "Wired", "--as", "json", "-q"]);
        out.matches(r#""relationship":"#).count()
    };

    // The first box has nothing to connect to yet.
    let (code, out, _) = m.run(&["view", "add", "Wired", "Svc"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(edges(&m), 0);

    // The second completes a relationship that is already in the model.
    let (code, _, err) = m.run(&["view", "add", "Wired", "BA1"]);
    assert_eq!(code, 0);
    assert_eq!(edges(&m), 1, "the Serving relationship was not drawn: {err}");

    // Re-adding does not draw it twice.
    assert_eq!(m.run(&["view", "add", "Wired", "BA1"]).0, 0);
    assert_eq!(edges(&m), 1, "a second copy of the connection was written");

    // Opting out still works, and the model stays loadable throughout.
    assert_eq!(m.run(&["element", "add", "DataObject", "Rec"]).0, 0);
    assert_eq!(m.run(&["relation", "add", "Access", "Svc", "Rec"]).0, 0);
    assert_eq!(m.run(&["view", "add", "Wired", "Rec", "--no-connect"]).0, 0);
    assert_eq!(edges(&m), 1, "--no-connect drew a connection anyway");
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// The write side takes `Triggering`; the query side took only
/// `TriggeringRelationship` and answered 0 for the other, which reads as a fact
/// about the model rather than as a vocabulary mismatch.
#[test]
fn type_filters_take_the_archimate_name_and_reject_what_is_not_a_type() {
    let m = Model::new("testmodel1.archimate");
    let count = |args: &[&str]| -> String {
        let (_, out, _) = m.run(args);
        out.trim().to_string()
    };
    assert_eq!(count(&["query", "type=AssignmentRelationship", "--count"]), "1");
    assert_eq!(count(&["query", "type=Assignment", "--count"]), "1", "the ArchiMate spelling");
    assert_eq!(count(&["list", "-t", "Assignment", "--count"]), "1", "and on -t too");

    // A type that does not exist is a mistake, not an empty result set.
    let (code, _, err) = m.run(&["list", "-t", "NotAType", "--count"]);
    assert_eq!(code, 2, "usage");
    assert!(err.contains("is not a concept type"), "{err}");
    assert!(err.contains("AssignmentRelationship"), "the model's own types are listed: {err}");

    // `-t element` is the category mistake, and there is now a field for it.
    let (code, _, err) = m.run(&["list", "-t", "element", "--count"]);
    assert_eq!(code, 2);
    assert!(err.contains("kind=element"), "points at the filter field: {err}");

    // Which is what separates relationships from elements in a query.
    assert_eq!(count(&["query", "kind=relation", "--count"]), "1");
    assert_eq!(count(&["query", "kind=element", "--count"]), "2");
}

/// `view~"Name"` filtered but the column was always empty and `view=0` matched
/// nothing, so "which concepts are on no view" — the invariant a model built
/// this way depends on — could not be asked at all.
#[test]
fn the_view_field_reports_how_many_and_which() {
    let m = Model::new("testmodel1.archimate");
    assert_eq!(m.run(&["element", "add", "Goal", "Undrawn"]).0, 0);

    let count = |args: &[&str]| -> String {
        let (_, out, _) = m.run(args);
        out.trim().to_string()
    };
    assert_eq!(count(&["query", "view=0", "--count"]), "1", "the element on no view");
    assert_eq!(count(&["query", "view<1", "--count"]), "1");
    assert_eq!(count(&["query", "name=Undrawn", "--fields", "name,views"]), "Undrawn\t0");

    // A field that does not exist projected to nothing and said nothing, so a
    // near-miss spelling read as "this model has no view information". On a
    // read it is a usage error now, before any row.
    let (code, out, err) = m.run(&["list", "-l", "1", "--fields", "name,view"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no such field: view"), "{err}");
    assert!(err.contains("views"), "the real column is named: {err}");
    assert!(out.is_empty(), "no row was printed under a wrong projection: {out}");

    // A name still filters by view, and the count column agrees with it.
    let (_, out, _) = m.run(&["query", "view~\"2 Test\"", "--fields", "name,views", "-q"]);
    assert!(!out.is_empty(), "the view name filter stopped working");
    for row in rows(&out) {
        assert_ne!(row[1], "0", "on a view but counted as on none: {row:?}");
    }
}

/// A field you can filter on is a field you can print.
///
/// `--fields` was a filter over the columns a command had already chosen, so
/// `--fields name,prop:reg-id` — asked straight after `query 'prop:reg-id=…'`
/// had matched on that field — projected the column away, said "no such
/// field" on stderr, and left reading one property to fetching the whole
/// record as JSON.
#[test]
fn a_projection_can_ask_for_what_the_record_does_not_print() {
    let m = Model::new("testmodel1.archimate");
    assert_eq!(m.run(&["element", "add", "Goal", "Ledger", "--doc", "Where the money is."]).0, 0);
    assert_eq!(m.run(&["prop", "set", "Ledger", "reg-id", "RG-14"]).0, 0);

    let (code, out, err) = m.run(&["query", "prop:reg-id=RG-14", "--fields", "name,prop:reg-id"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "Ledger\tRG-14", "the property it just filtered on: {out}");
    assert!(!err.contains("no such field"), "{err}");
    assert!(err.contains("prop:reg-id"), "the header names the column: {err}");

    // Documentation, layer and kind are on the concept and too big or too rare
    // for every row; asked for, they come.
    let (_, out, _) = m.run(&["query", "name=Ledger", "--fields", "name,kind,layer,doc", "-q"]);
    assert_eq!(out.trim(), "Ledger\telement\tMotivation\tWhere the money is.");

    // A property nothing carries is an empty column, not a missing one: a
    // dropped column is what made the miss silent in the first place.
    let (_, out, err) = m.run(&["query", "name=Ledger", "--fields", "name,prop:nobody", "-q"]);
    assert_eq!(out.trim_end_matches('\n'), "Ledger\t", "an absent property is an empty column");
    assert!(!err.contains("no such field"), "{err}");

    // A command that prints a column of its own keeps it: `trace` writes
    // `kind` to tell a node from an edge.
    let (_, out, _) = m.run(&["trace", "Ledger", "-n", "1", "--fields", "kind,name", "-q"]);
    assert!(out.lines().all(|l| l.starts_with("node\t")), "trace kept its own kind: {out}");
}

/// A view carries documentation exactly as a concept does, and until this
/// there was no way in: `element doc` takes a concept, and a view is not one.
#[test]
fn a_view_has_documentation() {
    let m = Model::new("testmodel1.archimate");
    let (_, out, _) = m.run(&["view", "list", "--fields", "id", "-q"]);
    let view = out.lines().next().unwrap().trim().to_string();

    let before = std::fs::read(m.path()).unwrap();
    let (code, _, err) = m.run(&["view", "doc", &view, "What this drawing is for."]);
    assert_eq!(code, 0, "{err}");

    let (_, out, _) = m.run(&["view", "list", "--fields", "id,doc", "-q"]);
    let row = out.lines().find(|l| l.starts_with(&view)).unwrap();
    assert_eq!(row.trim(), format!("{view}\tWhat this drawing is for."));

    // An empty string removes it, and removing it puts the file back exactly
    // as it was — the round trip this whole tool stands on.
    assert_eq!(m.run(&["view", "doc", &view, ""]).0, 0);
    assert_eq!(std::fs::read(m.path()).unwrap(), before, "clearing left the file changed");
}

/// Truncation is not commentary, so `-q` may not silence it.
///
/// `-q` drops the header and the notes, which are decoration. It also dropped
/// "83 total, showing 50", and four commands never said it at all — so an
/// agent counting by type got fifty of eighty-three and no way to know it,
/// which is not a smaller answer but a wrong one.
#[test]
fn a_capped_answer_says_so_whatever_the_flags() {
    let m = Model::new("testmodel1.archimate");
    // `neighbors` is one of the four that used to truncate in silence, and the
    // fixture's actor has a single neighbour — one more, and a cap of one cuts.
    assert_eq!(m.run(&["element", "add", "BusinessRole", "Second Role"]).0, 0);
    assert_eq!(m.run(&["relation", "add", "Assignment", "Business Actor", "Second Role"]).0, 0);

    for args in [
        &["query", "kind=element", "-l", "1", "-q"][..],
        &["list", "-l", "1", "-q"][..],
        &["search", "e", "-l", "1", "-q"][..],
        &["neighbors", "Business Actor", "-l", "1", "-q"][..],
    ]
    .into_iter()
    {
        let (code, out, err) = m.run(args);
        assert_eq!(code, 0, "{err}");
        assert_eq!(out.lines().count(), 1, "{args:?} printed more than the cap");
        assert!(err.contains("showing 1 of"), "{args:?} truncated in silence: {err:?}");
        assert!(err.contains("-l 0"), "{args:?} did not say how to see the rest: {err:?}");
    }

    // Uncapped, it says nothing: a caveat that is always there is noise.
    let (_, _, err) = m.run(&["query", "kind=element", "-l", "0", "-q"]);
    assert!(!err.contains("showing"), "warned about a complete answer: {err:?}");

    // The envelope keeps saying it too, for a reader that parses rather than
    // reads.
    let (_, out, _) = m.run(&["query", "kind=element", "-l", "2", "-F", "json"]);
    assert!(out.contains(r#""truncated":true"#), "{out}");
}

/// An unknown flag used to end with "this amcli is older than that document","
/// which sent a reader off to reinstall a current binary over a misremembered
/// flag name. An unknown *subcommand* is the case that footer is for.
#[test]
fn an_unknown_flag_names_the_flags_instead_of_blaming_the_binary() {
    let m = Model::new("testmodel1.archimate");
    let (code, _, err) = m.run(&["view", "layout", "0 Blank View", "--bogus"]);
    assert_eq!(code, 2);
    assert!(!err.contains("older"), "an unknown flag is not version skew: {err}");
    assert!(err.contains("--relayout-all"), "the command's own flags are listed: {err}");
    assert!(err.contains("--model"), "and the global ones: {err}");

    // A missing file is not version skew either.
    let out = Command::cargo_bin("amcli")
        .unwrap()
        .args(["-m", " /nope.archimate", "info"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("older"), "{err}");
    // Quoted, or a leading space from an unsplit shell variable is invisible.
    assert!(err.contains("` /nope.archimate`"), "the path is not quoted: {err}");
}

/// `view auto --layout` and `view layout --algorithm` named the same concept two
/// ways, and guessing wrong produced an error that looked like a missing command.
#[test]
fn either_spelling_of_the_layout_flag_is_accepted() {
    let m = Model::new("modelimporter_test.archimate");
    for args in [
        &["view", "auto", "A", "--from", "BA1", "--layout", "grid"][..],
        &["view", "auto", "B", "--from", "BA1", "--algorithm", "grid"][..],
    ] {
        assert_eq!(m.run(args).0, 0, "{args:?}");
    }
    for flag in ["--algorithm", "--layout"] {
        let (code, out, err) = m.run(&["view", "layout", "A", flag, "grid", "--relayout-all"]);
        assert_eq!(code, 0, "{flag}: {err}");
        assert!(out.contains("grid"), "the algorithm used is reported: {out}");
    }

    // And a name that is not an algorithm lists the ones that are.
    let (code, _, err) = m.run(&["view", "layout", "A", "--layout", "spiral"]);
    assert_eq!(code, 2);
    assert!(err.contains("grid"), "{err}");
}

/// Two builds reporting the same version cannot be told apart, which is what
/// made a stale binary earlier in PATH look like a broken skill.
///
/// The version comes from the package rather than being spelled out here: this
/// test is about the build identifier, and hard-coding the number only means it
/// fails on the commit that bumps it.
#[test]
fn the_version_says_which_build_it_is() {
    let out = Command::cargo_bin("amcli").unwrap().arg("--version").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let expected = format!("amcli {}", env!("CARGO_PKG_VERSION"));
    assert!(text.starts_with(&expected), "expected {expected}, got {text}");
    assert!(text.contains('('), "no build identifier: {text}");
    // Whatever it is, it is not empty parentheses.
    let build = text.split('(').nth(1).unwrap().trim_end_matches([')', '\n']);
    assert!(build.len() > 3, "the build identifier is empty: {text}");
}

/// The columns were `<id> <name> <type?> <n> <n> <n>` and had to be guessed at.
/// Naming them on stdout would break `cut -f2`, so they are named on stderr.
#[test]
fn records_carry_a_column_header_on_stderr() {
    let m = Model::new("testmodel1.archimate");
    let (code, out, err) = m.run(&["view", "list"]);
    assert_eq!(code, 0);
    assert!(err.contains("# id\tname"), "the columns are named: {err}");
    for line in out.lines() {
        assert!(!line.starts_with('#'), "the header leaked into the data: {line}");
    }

    // -q is still nothing but records.
    let (_, _, err) = m.run(&["view", "list", "-q"]);
    assert!(!err.contains('#'), "-q asked for no envelope: {err}");

    // A command returning two record shapes labels both.
    let (_, _, err) = m.run(&["trace", "Business Actor", "-n", "2"]);
    assert_eq!(err.matches('#').count(), 2, "nodes and edges are labelled separately: {err}");
}

/// Creating a model meant hand-writing XML, which is the one thing the skill
/// tells an agent never to do.
#[test]
fn init_creates_a_model_the_rest_of_the_tool_can_use() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.archimate");
    let amcli = |args: &[&str]| Command::cargo_bin("amcli").unwrap().args(args).output().unwrap();

    let out = amcli(&["init", "Monetech & Co", "-o", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(path.exists());

    let p = path.to_str().unwrap();
    // Every folder a write needs is there, so the normal loop works immediately.
    for args in [
        &["-m", p, "element", "add", "ApplicationComponent", "Svc"][..],
        &["-m", p, "element", "add", "DataObject", "Rec"][..],
        &["-m", p, "relation", "add", "Access", "Svc", "Rec", "--access", "rw"][..],
        &["-m", p, "view", "auto", "V", "--from", "Svc"][..],
        &["-m", p, "validate"][..],
    ] {
        let out = amcli(args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // The name survived escaping, which is why this is not a format! template.
    let out = amcli(&["-m", p, "info", "-F", "json", "-q"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Monetech & Co"));

    // An existing file is not silently overwritten.
    assert_eq!(amcli(&["init", "Other", "-o", p]).status.code(), Some(6), "conflict");
    assert_eq!(amcli(&["init", "Other", "-o", p, "--force"]).status.code(), Some(0));
}

/// Found by rebuilding a real model twice: same size, same content, different
/// property order. `HashMap` iteration is randomised per process, so a batch
/// applied twice wrote the properties in a different order each time and the
/// rebuild still produced a diff — deterministic ids do not help if the lines
/// around them move.
#[test]
fn properties_from_a_batch_are_written_in_a_stable_order() {
    let m = Model::new("modelimporter_test.archimate");
    let ops = m.dir.path().join("ops.jsonl");
    let keys = ["owner", "tier", "zone", "cost", "sla", "team"];
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"element.add","type":"ApplicationComponent","name":"Svc","props":"#,
            r#"{"owner":"a","tier":"1","zone":"eu","cost":"9","sla":"gold","team":"x"}}"#,
            "\n",
        ),
    )
    .unwrap();
    assert_eq!(m.run(&["apply", ops.to_str().unwrap()]).0, 0);

    // Key order, which is a property of one run rather than a comparison between
    // two: a comparison would pass by luck one time in 720.
    let text = m.text();
    let at = |k: &str| text.find(&format!(r#"key="{k}""#)).unwrap_or_else(|| panic!("no {k}"));
    let mut sorted = keys;
    sorted.sort_unstable();
    let positions: Vec<usize> = sorted.iter().map(|k| at(k)).collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "properties are not in key order: {positions:?}"
    );
}

/// Rebuilding from identical batches regenerated every id, so a semantically
/// unchanged model produced a whole-file diff.
#[test]
fn a_seed_makes_a_rebuild_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let amcli = |args: &[&str]| Command::cargo_bin("amcli").unwrap().args(args).output().unwrap();

    let build = |name: &str, seed: Option<&str>| -> Vec<u8> {
        let path = dir.path().join(name);
        let p = path.to_str().unwrap().to_string();
        let mut steps: Vec<Vec<String>> = vec![
            vec!["init".into(), "Seeded".into(), "-o".into(), p.clone()],
            vec![
                "-m".into(),
                p.clone(),
                "element".into(),
                "add".into(),
                "ApplicationComponent".into(),
                "Svc".into(),
            ],
            vec![
                "-m".into(),
                p.clone(),
                "element".into(),
                "add".into(),
                "DataObject".into(),
                "Rec".into(),
            ],
            vec![
                "-m".into(),
                p.clone(),
                "relation".into(),
                "add".into(),
                "Access".into(),
                "Svc".into(),
                "Rec".into(),
            ],
            vec![
                "-m".into(),
                p.clone(),
                "view".into(),
                "auto".into(),
                "V".into(),
                "--from".into(),
                "Svc".into(),
            ],
        ];
        for step in &mut steps {
            if let Some(s) = seed {
                step.push("--id-seed".into());
                step.push(s.into());
            }
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            let out = amcli(&args);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::fs::read(&path).unwrap()
    };

    assert_eq!(
        build("a.archimate", Some("demo")),
        build("b.archimate", Some("demo")),
        "the same model built twice with the same seed differs"
    );
    // Random stays the default: deriving an id from a name would give the same
    // id to two models that both contain "Payment API".
    assert_ne!(build("c.archimate", None), build("d.archimate", None));
}

/// The layout's whole job, asserted end to end on a graph that admits a clean
/// drawing: no bendpoints, no segment through a box, and no two segments
/// crossing each other.
///
/// This graph is seven nodes and seven edges — a tree plus one cycle — so it is
/// planar and a perfect drawing exists. Producing anything worse would mean the
/// layout is inventing difficulty.
#[test]
fn a_graph_that_can_be_drawn_cleanly_is_drawn_cleanly() {
    let m = Model::new("modelimporter_test.archimate");
    for (ty, name) in [
        ("ApplicationComponent", "Payment API"),
        ("ApplicationService", "Card Authorization"),
        ("ApplicationFunction", "Authorize"),
        ("DataObject", "Payment Record"),
        ("Goal", "Reduce fraud"),
    ] {
        assert_eq!(m.run(&["element", "add", ty, name]).0, 0);
    }
    for (ty, a, b) in [
        ("Assignment", "Payment API", "Authorize"),
        ("Access", "Authorize", "Payment Record"),
        ("Realization", "Authorize", "Card Authorization"),
        ("Serving", "Card Authorization", "BR1"),
        ("Influence", "Card Authorization", "Reduce fraud"),
        ("Serving", "Payment API", "BR1"),
    ] {
        assert_eq!(m.run(&["relation", "add", ty, a, b]).0, 0, "{ty} {a} -> {b}");
    }

    assert_eq!(m.run(&["view", "auto", "V", "--from", "Payment API", "-n", "4"]).0, 0);
    let (code, out, _) = m.run(&["view", "render", "V", "--as", "json"]);
    assert_eq!(code, 0);

    let (boxes, lines) = scene(&out);
    assert!(boxes.len() >= 6, "parsed {} boxes from {out}", boxes.len());
    assert!(lines.len() >= 6, "parsed {} edges from {out}", lines.len());

    let bends: usize = lines.iter().map(|l| l.len().saturating_sub(2)).sum();
    assert_eq!(bends, 0, "this graph needs no bendpoints at all");

    let mut through = 0;
    for line in &lines {
        for (p, q) in line.iter().zip(line.iter().skip(1)) {
            for b in &boxes {
                if segment_enters(*p, *q, *b) {
                    through += 1;
                }
            }
        }
    }
    assert_eq!(through, 0, "{through} segments run through a box");

    let segments: Vec<((i32, i32), (i32, i32))> =
        lines.iter().flat_map(|l| l.iter().zip(l.iter().skip(1)).map(|(a, b)| (*a, *b))).collect();
    let mut crossings = 0;
    for (i, (a, b)) in segments.iter().enumerate() {
        for (c, d) in segments.iter().skip(i + 1) {
            if segments_cross(*a, *b, *c, *d) {
                crossings += 1;
            }
        }
    }
    assert_eq!(crossings, 0, "{crossings} pairs of edges cross");

    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

type Boxes = Vec<(i32, i32, i32, i32)>;
type Lines = Vec<Vec<(i32, i32)>>;

/// A minimal read of the scene dump: enough to walk segments against boxes.
fn scene(out: &str) -> (Boxes, Lines) {
    let boxes: Boxes = out
        .split(r#"{"id":"#)
        .filter(|s| s.contains(r#""depth""#))
        .filter_map(|s| {
            let n = |k: &str| -> Option<i32> {
                s.split(&format!(r#""{k}":"#)).nth(1)?.split([',', '}']).next()?.parse().ok()
            };
            Some((n("x")?, n("y")?, n("w")?, n("h")?))
        })
        .collect();

    let lines: Lines = out
        .split(r#""points":[["#)
        .skip(1)
        .map(|s| {
            s.split("]]")
                .next()
                .unwrap_or_default()
                .split("],[")
                .filter_map(|p| {
                    let mut it = p.trim_matches(['[', ']']).split(',');
                    Some((it.next()?.trim().parse().ok()?, it.next()?.trim().parse().ok()?))
                })
                .collect()
        })
        .collect();
    (boxes, lines)
}

/// Does the segment pass through the interior of the box? The box is inset a
/// little, because an endpoint resting on its own border is normal.
fn segment_enters(p: (i32, i32), q: (i32, i32), b: (i32, i32, i32, i32)) -> bool {
    let (x, y, w, h) = b;
    for step in 1..60 {
        let t = step as f64 / 60.0;
        let px = p.0 as f64 + (q.0 - p.0) as f64 * t;
        let py = p.1 as f64 + (q.1 - p.1) as f64 * t;
        if px > (x + 2) as f64
            && px < (x + w - 2) as f64
            && py > (y + 2) as f64
            && py < (y + h - 2) as f64
        {
            return true;
        }
    }
    false
}

fn segments_cross(a: (i32, i32), b: (i32, i32), c: (i32, i32), d: (i32, i32)) -> bool {
    // Segments meeting at a shared endpoint are edges leaving the same box, not
    // a crossing.
    let ends = [a, b, c, d];
    if ends.iter().enumerate().any(|(i, p)| ends.iter().skip(i + 1).any(|q| p == q)) {
        return false;
    }
    let orient = |p: (i32, i32), q: (i32, i32), r: (i32, i32)| -> i64 {
        (q.1 - p.1) as i64 * (r.0 - q.0) as i64 - (q.0 - p.0) as i64 * (r.1 - q.1) as i64
    };
    let sign = |v: i64| v.signum();
    sign(orient(a, b, c)) != sign(orient(a, b, d)) && sign(orient(c, d, a)) != sign(orient(c, d, b))
}

/// Views are filed in folders, and a folder is checked before a view is made.
///
/// Every view landing at the top of `/Views` is fine for ten views and useless
/// for thirty, so `create`, `auto` and the batch all take a folder, and `move`
/// re-files the ones already there. The destination is checked first: a view
/// filed outside the views tree parses but never appears in Archi, which is the
/// kind of breakage that is only noticed by the person who opens the model.
/// A viewpoint could only be chosen when a view was created, so a drawing that
/// grew past the one it was filed under could not be corrected without deleting
/// and rebuilding it. Setting one afterwards has to hold the same two promises
/// every other write does: an unknown id is refused before anything is touched,
/// and clearing what was set leaves the file byte-identical.
#[test]
fn a_views_viewpoint_can_be_set_after_it_exists() {
    let m = Model::new("modelimporter_test.archimate");
    let before = std::fs::read(m.path()).unwrap();
    let viewpoint_of = |m: &Model, name: &str| -> String {
        let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name,viewpoint"]);
        rows(&out)
            .iter()
            .find(|r| r.first() == Some(&name))
            .and_then(|r| r.get(1))
            .unwrap_or(&"")
            .to_string()
    };

    assert_eq!(m.run(&["view", "create", "Scope"]).0, 0);
    assert_eq!(viewpoint_of(&m, "Scope"), "", "a view starts with no viewpoint");

    // An id that is not a viewpoint is a usage error, and the hint lists the
    // ones that are.
    let (code, _, err) = m.run(&["view", "viewpoint", "Scope", "not_a_viewpoint"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not a viewpoint id"), "{err}");
    assert!(err.contains("layered"), "the hint names the real ones: {err}");
    assert_eq!(viewpoint_of(&m, "Scope"), "", "the refusal changed nothing");

    let (code, out, err) = m.run(&["view", "viewpoint", "Scope", "layered"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("layered"), "{out}");
    assert_eq!(viewpoint_of(&m, "Scope"), "layered");

    // Changing it again reports where it came from.
    let (_, out, _) = m.run(&["view", "viewpoint", "-q", "Scope", "strategy"]);
    let r = rows(&out);
    assert_eq!(r[0][2], "layered", "reports the old value: {out}");
    assert_eq!(r[0][3], "strategy", "reports the new one: {out}");

    // The same op in a batch, and then cleared.
    let ops = "{\"op\":\"view.viewpoint\",\"view\":\"Scope\",\"viewpoint\":\"motivation\"}\n";
    let batch = m.path().with_file_name("vp.jsonl");
    std::fs::write(&batch, ops).unwrap();
    assert_eq!(m.run(&["apply", batch.to_str().unwrap()]).0, 0);
    assert_eq!(viewpoint_of(&m, "Scope"), "motivation");

    assert_eq!(m.run(&["view", "viewpoint", "Scope", ""]).0, 0);
    assert_eq!(viewpoint_of(&m, "Scope"), "", "an empty viewpoint clears it");

    // EMF omits the attribute when there is no viewpoint, so a view that has
    // been given one and had it taken away is the file it started as.
    assert_eq!(m.run(&["view", "delete", "Scope"]).0, 0);
    assert_eq!(std::fs::read(m.path()).unwrap(), before, "set then cleared is not byte-identical");
}

#[test]
fn views_are_filed_in_folders() {
    let m = Model::new("modelimporter_test.archimate");
    let folder_of = |m: &Model, name: &str| -> String {
        let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name,folder"]);
        rows(&out)
            .iter()
            .find(|r| r.first() == Some(&name))
            .and_then(|r| r.get(1))
            .unwrap_or(&"")
            .to_string()
    };

    // The folder has to exist first — `folder add` is what makes one.
    let (code, _, err) = m.run(&["view", "create", "Filed", "-f", "/Views/Motivation"]);
    assert_eq!(code, 3, "a folder that does not exist is not found: {err}");
    assert!(err.contains("no folder at `/Views/Motivation`"), "{err}");
    assert_eq!(named_views(&m, "Filed"), 0, "nothing was created behind the error");

    assert_eq!(m.run(&["folder", "add", "/Views", "Motivation"]).0, 0);
    assert_eq!(m.run(&["view", "create", "Filed", "-f", "/Views/Motivation"]).0, 0);
    assert_eq!(folder_of(&m, "Filed"), "/Views/Motivation");

    // A folder outside the views tree is refused, not silently obeyed: Archi
    // shows no diagram filed under /Business.
    let (code, _, err) = m.run(&["view", "create", "Stray", "-f", "/Business"]);
    assert_eq!(code, 5, "{err}");
    assert!(err.contains("not under the views folder"), "{err}");

    // An existing view moves, and reports where it came from.
    assert_eq!(m.run(&["view", "create", "Loose"]).0, 0);
    assert_eq!(folder_of(&m, "Loose"), "/Views");
    let (code, out, _) = m.run(&["view", "move", "Loose", "-f", "/Views/Motivation"]);
    assert_eq!(code, 0);
    assert!(out.contains("/Views\t/Views/Motivation"), "from and to: {out}");
    assert_eq!(folder_of(&m, "Loose"), "/Views/Motivation");

    // Moving somewhere it already is changes nothing and is not an error, so a
    // regenerate-everything script stays re-runnable.
    assert_eq!(m.run(&["view", "move", "Loose", "-f", "/Views/Motivation"]).0, 0);
    assert_eq!(folder_of(&m, "Loose"), "/Views/Motivation");

    // `view auto` and the batch file views the same way.
    assert_eq!(
        m.run(&["view", "auto", "Neighbourhood", "--from", "BA1", "-f", "/Views/Motivation"]).0,
        0
    );
    assert_eq!(folder_of(&m, "Neighbourhood"), "/Views/Motivation");

    let ops = m.dir.path().join("folders.jsonl");
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"folder.add","parent":"/Views","name":"Programme"}"#,
            "\n",
            r#"{"op":"view.create","name":"Batched","folder":"/Views/Programme","replace":true}"#,
            "\n",
            r#"{"op":"view.move","view":"Filed","folder":"/Views/Programme"}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, out, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("view.move"), "{out}");
    assert_eq!(folder_of(&m, "Batched"), "/Views/Programme");
    assert_eq!(folder_of(&m, "Filed"), "/Views/Programme");

    // A batch that names a bad folder writes nothing at all.
    let model_file = m.dir.path().join("m.archimate");
    let before = std::fs::read(&model_file).unwrap();
    let bad = m.dir.path().join("bad.jsonl");
    std::fs::write(
        &bad,
        concat!(
            r#"{"op":"view.create","name":"Half","folder":"/Views/Programme","replace":true}"#,
            "\n",
            r#"{"op":"view.move","view":"Batched","folder":"/Nowhere"}"#,
            "\n",
        ),
    )
    .unwrap();
    assert_eq!(m.run(&["apply", bad.to_str().unwrap()]).0, 3);
    assert_eq!(std::fs::read(&model_file).unwrap(), before, "the file is byte-identical");

    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// Declaring a folder twice gives one folder, not two.
///
/// This is the shape every regenerate-everything script has — declare the
/// folders, then file the views — so a `folder add` that appended a second
/// folder of the same name turned each re-run into another duplicate, three
/// deep before anyone opened Archi and saw them. `folder_by_path` can only
/// return one of them, so the extras are not even reachable to fix.
#[test]
fn declaring_a_folder_twice_gives_one_folder() {
    let m = Model::new("modelimporter_test.archimate");
    let folders = || -> usize {
        let (_, out, _) = m.run(&["folder", "list", "-q", "--fields", "path"]);
        rows(&out).iter().filter(|r| r.first() == Some(&"/Views/Programme")).count()
    };

    let (code, out, _) = m.run(&["folder", "add", "/Views", "Programme"]);
    assert_eq!(code, 0);
    assert!(out.contains("true"), "reports that it created one: {out}");
    assert_eq!(folders(), 1);

    let (code, out, _) = m.run(&["folder", "add", "/Views", "Programme"]);
    assert_eq!(code, 0, "a repeat is not an error");
    assert!(out.contains("false"), "reports that it created nothing: {out}");
    assert_eq!(folders(), 1, "still one folder, not two");

    // A view filed there survives the repeat, because the folder is the same one.
    assert_eq!(m.run(&["view", "create", "Filed", "-f", "/Views/Programme"]).0, 0);
    assert_eq!(m.run(&["folder", "add", "/Views", "Programme"]).0, 0);
    let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name,folder"]);
    assert!(out.contains("Filed\t/Views/Programme"), "{out}");

    // An empty folder can be removed; one holding something cannot.
    let (code, _, err) = m.run(&["folder", "delete", "/Views/Programme"]);
    assert_eq!(code, 5, "refuses while the view is in it: {err}");
    assert!(err.contains("still holds 1"), "{err}");

    assert_eq!(m.run(&["view", "delete", "Filed"]).0, 0);
    assert_eq!(m.run(&["folder", "delete", "/Views/Programme"]).0, 0);
    assert_eq!(folders(), 0);

    // The nine Archi expects are not deletable.
    let (code, _, err) = m.run(&["folder", "delete", "/Views"]);
    assert_eq!(code, 5, "{err}");

    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// `export views` and `apply` are inverses, and stay inverses.
///
/// A view has no declarative form in the file — what it holds is only geometry
/// — so "which elements are on this view, and why those" is not a question a
/// diff can answer. Keeping member lists beside the model answers it and
/// invents a second source of truth that goes stale; deriving them from the
/// model does not. That only works if the round trip is exact, so this asserts
/// byte identity rather than "looks right", and asserts it twice: an export
/// that reorders the views it rebuilds would still pass a one-shot check while
/// making every regeneration churn the whole file.
#[test]
fn exported_views_rebuild_the_model_byte_for_byte() {
    let m = Model::new("modelimporter_test.archimate");
    let model_file = m.dir.path().join("m.archimate");

    // The fixture's own views were drawn in Archi and hold notes and nested
    // objects, which `view.add` cannot put back — the export says so in a
    // comment rather than pretending otherwise. The round trip is exact for
    // views amcli built, which is what a regenerated model is made of.
    for stale in ["View 1", "View 2"] {
        assert_eq!(m.run(&["view", "delete", stale, "-y"]).0, 0);
    }
    // Seeded, because that is the only way a rebuild can be byte-identical:
    // without it every recreated view draws a fresh random id. Each `run` is
    // its own process, so the seed does not leak into the other tests here.
    let seed = ["--id-seed", "roundtrip"];
    let seeded = |args: &[&str]| -> (i32, String, String) {
        let mut all = args.to_vec();
        all.extend_from_slice(&seed);
        m.run(&all)
    };

    assert_eq!(seeded(&["folder", "add", "/Views", "Group"]).0, 0);
    for name in ["V1", "V2", "V3", "V4", "V5"] {
        assert_eq!(seeded(&["view", "create", name, "-f", "/Views/Group"]).0, 0);
        assert_eq!(seeded(&["view", "add", name, "BA1"]).0, 0);
    }

    let spec = m.dir.path().join("views.jsonl");
    let (code, _, err) = m.run(&["export", "views", "-o", spec.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let text = std::fs::read_to_string(&spec).unwrap();
    assert!(text.contains(r#""op":"folder.add""#), "declares its folders: {text}");
    assert!(text.contains(r#""folder":"/Views/Group""#), "files the views: {text}");
    assert!(text.contains("# V3"), "readable, one comment per view: {text}");

    let before = std::fs::read_to_string(&model_file).unwrap();
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    let once = std::fs::read_to_string(&model_file).unwrap();
    assert_eq!(first_difference(&before, &once), None, "one round trip changes nothing");

    // Twice, because a rebuild that reorders is stable only on odd passes.
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    let twice = std::fs::read_to_string(&model_file).unwrap();
    assert_eq!(first_difference(&before, &twice), None, "and neither does a second");

    // And the spec is the same spec, so it can be reviewed in a diff.
    let again = m.dir.path().join("views2.jsonl");
    assert_eq!(m.run(&["export", "views", "-o", again.to_str().unwrap()]).0, 0);
    assert_eq!(std::fs::read_to_string(&again).unwrap(), text);

    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// A replaced view is rebuilt where it was, not appended.
///
/// `--replace` deletes and recreates, and a recreated view used to land at the
/// end of its folder. With three views nobody notices; with thirty, every
/// regeneration rewrites the whole views section and the diff stops being
/// worth reading — which is the one thing this tool exists to protect.
#[test]
fn replacing_a_view_keeps_its_place_in_the_folder() {
    let m = Model::new("modelimporter_test.archimate");
    let order = |m: &Model| -> Vec<String> {
        let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name"]);
        rows(&out).iter().filter_map(|r| r.first()).map(|s| s.to_string()).collect()
    };

    assert_eq!(m.run(&["folder", "add", "/Views", "Group"]).0, 0);
    for name in ["V1", "V2", "V3"] {
        assert_eq!(m.run(&["view", "create", name, "-f", "/Views/Group"]).0, 0);
    }
    let before = order(&m);

    // One at a time, and in a batch — the batch is where it went wrong, because
    // the deleted node keeps its seat in the child list until the file is
    // written, so an index counted over live children pointed one place early.
    assert_eq!(m.run(&["view", "create", "V2", "-f", "/Views/Group", "--replace"]).0, 0);
    assert_eq!(order(&m), before, "a single replace holds the order");

    let ops = m.dir.path().join("all.jsonl");
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"view.create","name":"V1","folder":"/Views/Group","replace":true}"#,
            "\n",
            r#"{"op":"view.create","name":"V2","folder":"/Views/Group","replace":true}"#,
            "\n",
            r#"{"op":"view.create","name":"V3","folder":"/Views/Group","replace":true}"#,
            "\n",
        ),
    )
    .unwrap();
    assert_eq!(m.run(&["apply", ops.to_str().unwrap()]).0, 0);
    assert_eq!(order(&m), before, "and so does a batch that replaces every one");
}

/// The first line where two model files differ, for an assertion that has to
/// print something a person can read rather than half a megabyte of bytes.
fn first_difference(a: &str, b: &str) -> Option<String> {
    for (n, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            return Some(format!("line {}:\n  before: {x}\n   after: {y}", n + 1));
        }
    }
    (a.lines().count() != b.lines().count())
        .then(|| format!("{} lines before, {} after", a.lines().count(), b.lines().count()))
}

/// `-o v.png` is enough to ask for a raster; `--as png` says it outright.
#[test]
fn view_render_writes_png_when_asked_for_one() {
    let m = Model::new("testmodel1.archimate");
    let png = m.dir.path().join("v.png");
    let (code, _, err) =
        m.run(&["view", "render", "2 Test Bounds and Images", "-o", png.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let bytes = std::fs::read(&png).unwrap();
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");

    let out = Command::cargo_bin("amcli")
        .unwrap()
        .arg("-m")
        .arg(m.path())
        .args(["view", "render", "2 Test Bounds and Images", "--as", "png"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.starts_with(b"\x89PNG"), "png goes to stdout raw when there is no -o");
}

// ---- amcli web ---------------------------------------------------------------

/// The URL is the command's answer and has to be out before the server starts
/// serving: whoever launched the process reads one line and has the link. So
/// this spawns the real binary, reads stdout until the URL arrives, talks to
/// the server, and only then kills it.
#[test]
fn web_prints_its_url_before_it_serves() {
    use std::io::{BufRead, BufReader, Read, Write};
    let m = Model::new("testmodel1.archimate");
    let mut child = Command::cargo_bin("amcli")
        .unwrap()
        .arg("-m")
        .arg(m.path())
        .args(["web", "--no-open", "-F", "json", "-q"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let url = first
        .split("\"url\":\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or_else(|| panic!("no url in the first line: {first:?}"))
        .to_string();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let port: u16 =
        url.trim_start_matches("http://127.0.0.1:").trim_end_matches('/').parse().unwrap();

    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(format!("GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
        .unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).unwrap();
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(body.contains("\"checksum\":\""), "{body}");

    child.kill().unwrap();
    let _ = child.wait();
}

/// A port already taken is an error a person can act on, not a hang.
#[test]
fn web_refuses_a_busy_port_with_a_hint() {
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    let m = Model::new("testmodel1.archimate");
    let (code, _, err) = m.run(&["web", "--no-open", "--port", &port.to_string()]);
    assert_eq!(code, 7, "io: {err}");
    assert!(err.contains("--port"), "{err}");
}

/// Everything under `src/web/assets/` is compiled in by name. A file that is
/// there but not in the table would be silently unreachable, so this walks the
/// directory rather than trusting the list.
#[test]
fn every_web_asset_is_served() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/assets");
    let m = Model::new("testmodel1.archimate");
    let mut child = Command::cargo_bin("amcli")
        .unwrap()
        .arg("-m")
        .arg(m.path())
        .args(["web", "--no-open", "-q"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::{BufRead, BufReader, Read, Write};
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut first).unwrap();
    let port: u16 = first
        .split('\t')
        .next()
        .unwrap()
        .trim()
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/')
        .parse()
        .unwrap();

    let mut checked = 0;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            let url = if rel == "index.html" { "/".to_string() } else { format!("/{rel}") };
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("GET {url} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
                .unwrap();
            let mut body = Vec::new();
            s.read_to_end(&mut body).unwrap();
            let text = String::from_utf8_lossy(&body);
            assert!(
                text.starts_with("HTTP/1.1 200"),
                "{rel} is in src/web/assets but not served: {}",
                text.lines().next().unwrap_or("")
            );
            let want = std::fs::read(&path).unwrap();
            let got = &body[body.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4..];
            assert!(want == got, "{rel} differs between disk and the binary");
            checked += 1;
        }
    }
    assert!(checked >= 10, "walked only {checked} assets");
    child.kill().unwrap();
    let _ = child.wait();
}

/* ---- the design system's guardrails -------------------------------------------
The viewer's interface drifted once already: nine font sizes, twenty
spacings, four radius idioms and thirty-seven inline styles, each decided at
its own call site, plus three copies of the sortable table header with three
different ideas about which columns sort descending first. Care at the call
site is not what fixes that — it is what failed. These tests are. */

fn web_asset(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/assets").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn web_asset_paths(ext: &str) -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/assets");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// Strip `/* … */`, which is where the prose lives and where a hex may be
/// quoted while explaining why it is no longer used.
fn without_block_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("/*") {
        out.push_str(&rest[..at]);
        match rest[at..].find("*/") {
            Some(end) => rest = &rest[at + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `tokens.css` names every colour and every length; `app.css` may only refer
/// to them. A literal that slips into `app.css` is a decision taken at a call
/// site, which is how the interface came apart last time.
#[test]
fn tokens_are_the_only_literals() {
    let css = without_block_comments(&web_asset("app.css"));
    let mut sins = Vec::new();
    for (n, line) in css.lines().enumerate() {
        let no = n + 1;
        if let Some(at) = line.find('#') {
            let tail: String =
                line[at + 1..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if tail.len() >= 3 {
                sins.push(format!("app.css:{no}: colour literal #{tail} — name it in tokens.css"));
            }
        }
        if line.contains("rgb(") || line.contains("rgba(") {
            sins.push(format!("app.css:{no}: rgb() literal — name it in tokens.css"));
        }
        // A 1px hairline and a 0 are structural; every other length is a
        // decision, and decisions live in the token file.
        let bytes: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == 'p' && bytes[i + 1] == 'x' {
                let mut j = i;
                while j > 0 && (bytes[j - 1].is_ascii_digit() || bytes[j - 1] == '.') {
                    j -= 1;
                }
                let num: String = bytes[j..i].iter().collect();
                if !num.is_empty() && num != "0" && num != "1" {
                    sins.push(format!(
                        "app.css:{no}: length literal {num}px — name it in tokens.css"
                    ));
                }
            }
            i += 1;
        }
    }
    assert!(sins.is_empty(), "app.css must build from tokens only:\n  {}", sins.join("\n  "));
}

/// The same rule on the other side of the wire. A page module may compute a
/// length from data — a bar's width, a tree row's indent — but it may not
/// decide one: `style: { width: "220px" }` in three modules is how the viewer
/// ended up with three different widths for the same search box.
#[test]
fn page_modules_decide_no_lengths() {
    let mut sins = Vec::new();
    for (rel, src) in web_asset_paths("js") {
        for (n, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            for unit in ["px", "rem", "em"] {
                for quote in ['"', '\''] {
                    let needle = format!("{quote}");
                    let mut from = 0;
                    while let Some(at) = line[from..].find(&needle) {
                        let start = from + at + 1;
                        let lit: String = line[start..]
                            .chars()
                            .take_while(|c| c.is_ascii_digit() || *c == '.')
                            .collect();
                        if !lit.is_empty() && line[start + lit.len()..].starts_with(unit) {
                            sins.push(format!("{rel}:{}: hardcoded {lit}{unit}", n + 1));
                        }
                        from = start;
                    }
                }
            }
        }
    }
    assert!(
        sins.is_empty(),
        "a page module may compute a length but not decide one:\n  {}",
        sins.join("\n  ")
    );
}

/// One matcher, behind every search box. Each box used to decide for itself
/// what a match was, and all four decided the same thing — `includes` on a
/// lowercased name — which meant a reader had to spell a name the way the file
/// spells it, four times over. `fuzzy.js` is the answer now; a second copy of
/// the question is a matcher that was written at a call site again.
#[test]
fn one_matcher_behind_every_search() {
    let mut sins = Vec::new();
    for (rel, src) in web_asset_paths("js") {
        if rel == "fuzzy.js" {
            continue; // it is the matcher, and it says what it replaced
        }
        for (n, line) in src.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            if line.contains("toLowerCase().includes(") {
                sins.push(format!(
                    "{rel}:{}: a search of its own — use matches() from fuzzy.js",
                    n + 1
                ));
            }
        }
    }
    assert!(sins.is_empty(), "searching is fuzzy.js's job:\n  {}", sins.join("\n  "));
}

/// Chrome icons are drawn, not typed. Fifteen unicode characters used to stand
/// in for an icon set, each at the surrounding font's size, on its own
/// baseline, in whatever face the platform had — sitting on the same line as a
/// drawn ArchiMate figure.
#[test]
fn the_chrome_has_no_text_icons() {
    const RETIRED: &[char] = &['▣', '▶', '▼', '✕', '↗', '⤡', '◐', '‹', '›', '↔', '▾', '▴', '↕'];
    let mut sins = Vec::new();
    for (rel, src) in web_asset_paths("js") {
        if rel == "icons.js" {
            continue; // it names them, in a comment, to say what it replaced
        }
        for (n, line) in src.lines().enumerate() {
            for c in RETIRED {
                if line.contains(*c) {
                    sins.push(format!("{rel}:{}: `{c}` — use icon(\"…\") from icons.js", n + 1));
                }
            }
        }
    }
    let html = web_asset("index.html");
    for c in RETIRED {
        assert!(!html.contains(*c), "index.html still types `{c}` as an icon");
    }
    assert!(sins.is_empty(), "the icon set is icons.js:\n  {}", sins.join("\n  "));
}

/// Every foreground the palette offers, on every ground it is put on, clears
/// WCAG AA. The count inside a selected chip used to sit at 2.46:1 in dark,
/// because `.muted` beat the chip's inverted colour and nothing was watching.
#[test]
fn every_token_pair_clears_wcag_aa() {
    const PAIRS: &[(&str, &str)] = &[
        ("fg", "surface-0"),
        ("fg", "surface-1"),
        ("fg", "surface-2"),
        ("fg-muted", "surface-0"),
        ("fg-muted", "surface-1"),
        ("fg-muted", "surface-2"),
        ("fg-muted", "tint"),
        ("fg-subtle", "surface-0"),
        ("fg-subtle", "surface-1"),
        ("fg-subtle", "surface-2"),
        ("fg-subtle", "tint"),
        ("invert-fg", "invert"),
        ("invert-subtle", "invert"),
        ("alarm", "surface-1"),
        ("paper-ink", "paper"),
    ];
    let css = web_asset("tokens.css");
    let light = css.split("[data-theme=\"dark\"]").next().unwrap();
    let dark_block = css.split("[data-theme=\"dark\"]").nth(1).unwrap_or("");

    for (theme, block, fallback) in [("light", light, light), ("dark", dark_block, light)] {
        for (fg, bg) in PAIRS {
            let a = hex_token(block, fg).or_else(|| hex_token(fallback, fg));
            let b = hex_token(block, bg).or_else(|| hex_token(fallback, bg));
            let (a, b) = match (a, b) {
                (Some(a), Some(b)) => (a, b),
                _ => panic!("{theme}: tokens.css defines no --{fg} or --{bg}"),
            };
            let ratio = contrast(a, b);
            assert!(
                ratio >= 4.5,
                "{theme}: --{fg} on --{bg} is {ratio:.2}:1, below WCAG AA (4.5:1)"
            );
        }
    }

    fn hex_token(block: &str, name: &str) -> Option<[u8; 3]> {
        let needle = format!("--{name}:");
        let line = block.lines().find(|l| l.trim_start().starts_with(&needle))?;
        let at = line.find('#')?;
        let hex = &line[at + 1..at + 7];
        Some([
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ])
    }

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let l = |c: [u8; 3]| {
            let f = |v: u8| {
                let v = v as f64 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
        };
        let (x, y) = (l(a), l(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
}

/// The dark palette is written twice: once for `[data-theme="dark"]`, which is
/// what the toggle stamps, and once under `prefers-color-scheme` for the frame
/// before a deferred module has run. CSS has no way to share one block between
/// a selector and a media query, so this shares it — otherwise a reader whose
/// system is dark gets one palette until app.js starts and a slightly
/// different one after.
#[test]
fn the_two_dark_palettes_are_one_palette() {
    let css = web_asset("tokens.css");
    let stamped = declarations(&css, "[data-theme=\"dark\"]");
    let system = declarations(&css, ":root:not([data-theme=\"light\"])");
    assert!(!stamped.is_empty(), "tokens.css defines no [data-theme=\"dark\"] block");
    assert!(
        !system.is_empty(),
        "tokens.css has no prefers-color-scheme block: a dark reader gets a white page \
         until app.js runs"
    );
    assert_eq!(
        stamped, system,
        "the two dark blocks in tokens.css have drifted apart; every declaration must match"
    );

    /// Every `name: value` between the first `{` after `needle` and its `}`.
    fn declarations(css: &str, needle: &str) -> Vec<String> {
        let at = match css.find(needle) {
            Some(at) => at,
            None => return Vec::new(),
        };
        let open = at + css[at..].find('{').expect("a selector with no block");
        let close = open + css[open..].find('}').expect("a block that never closes");
        css[open + 1..close]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }
}

/// A relationship added between two elements a view already shows reached no
/// view, and nothing said so. The model then held a relationship drawn
/// nowhere, `view layout --relayout-all` could not add what was never in the
/// file, and the one way to draw it was to rebuild the view member by member
/// — a three-line fix became a forty-line batch every time.
#[test]
fn a_new_relationship_is_drawn_where_both_ends_already_are() {
    let m = Model::new("modelimporter_test.archimate");
    for (ty, name) in [("BusinessActor", "A"), ("Node", "N"), ("BusinessRole", "R")] {
        assert_eq!(m.run(&["element", "add", ty, name]).0, 0);
    }
    assert_eq!(m.run(&["view", "create", "V"]).0, 0);
    assert_eq!(m.run(&["view", "create", "W"]).0, 0);
    for name in ["A", "N", "R"] {
        assert_eq!(m.run(&["view", "add", "V", name]).0, 0);
    }
    assert_eq!(m.run(&["view", "add", "W", "A"]).0, 0);
    let edges = |view: &str| -> usize {
        let (_, out, _) = m.run(&["view", "render", view, "--as", "json", "-q"]);
        out.matches(r#""relationship":"#).count()
    };

    // Both ends on V: drawn there, and the row and the note say so. Only one
    // end on W: not drawn there.
    let (code, out, err) = m.run(&["relation", "add", "Assignment", "A", "R"]);
    assert_eq!(code, 0, "{err}");
    let row = rows(&out).remove(0);
    assert_eq!(row[4], "1", "the row counts the views it was drawn on: {out}");
    assert!(err.contains("drawn on 1 view(s): V"), "{err}");
    assert_eq!(edges("V"), 1);
    assert_eq!(edges("W"), 0);
    let (_, out, _) = m.run(&["query", "type=Assignment and name=''", "--fields", "views", "-q"]);
    assert!(out.lines().all(|l| l.trim() == "1"), "the relationship is on one view: {out}");

    // The same from a batch, and the opt-out on both.
    let ops = m.dir.path().join("rels.jsonl");
    std::fs::write(
        &ops,
        concat!(
            r#"{"op":"relation.add","type":"Association","source":"A","target":"N"}"#,
            "\n",
            r#"{"op":"relation.add","type":"Association","source":"N","target":"R","no_draw":true}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, out, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let r = rows(&out);
    assert_eq!(r[0][3], "1", "drawn: {out}");
    assert_eq!(r[1][3], "0", "no_draw: {out}");
    assert_eq!(edges("V"), 2);
    let (_, _, err) = m.run(&["relation", "add", "Serving", "N", "A", "--no-draw"]);
    assert!(!err.contains("drawn on"), "{err}");
    assert_eq!(edges("V"), 2);

    // Neither end drawn anywhere: silence, not a complaint about every write
    // on a model without views.
    assert_eq!(m.run(&["element", "add", "Goal", "Off"]).0, 0);
    let (_, out, err) = m.run(&["relation", "add", "Association", "Off", "R"]);
    assert!(err.contains("not drawn"), "one end is drawn, so it says why not: {err}");
    let _ = out;
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// `view add` of an element already on the view put a second box for it on
/// the drawing, exit 0, no note — so a batch that re-added a member to
/// "refresh" it corrupted the view, and `validate` was content. A present
/// member is left where it is; what it can newly draw is still drawn.
#[test]
fn adding_a_present_member_to_a_view_adds_nothing() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["element", "add", "ApplicationComponent", "Svc"]).0, 0);
    assert_eq!(m.run(&["view", "create", "V"]).0, 0);
    let nodes = || -> usize {
        let (_, out, _) = m.run(&["view", "render", "V", "--as", "json", "-q"]);
        out.matches(r#""concept":"#).count()
    };

    assert_eq!(m.run(&["view", "add", "V", "Svc"]).0, 0);
    let before = m.text();
    let (code, out, err) = m.run(&["view", "add", "V", "Svc"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(nodes(), 1, "a second box was drawn for the same element");
    let row = rows(&out).remove(0);
    assert_eq!(row[5], "false", "the row says nothing was added: {out}");
    assert!(err.contains("already on the view"), "{err}");
    assert_eq!(m.text(), before, "a no-op writes the same bytes");

    // Still wired: a relationship that reached the model without being drawn
    // is drawn by re-adding either end.
    assert_eq!(m.run(&["view", "add", "V", "BA1"]).0, 0);
    assert_eq!(m.run(&["relation", "add", "Serving", "Svc", "BA1", "--no-draw"]).0, 0);
    let (_, out, _) = m.run(&["view", "add", "V", "Svc", "-q"]);
    assert_eq!(rows(&out)[0][4], "1", "the missing line was drawn: {out}");
    assert_eq!(nodes(), 2);

    // And in a batch, so a rebuild batch is re-runnable: the second run adds
    // nothing and the file comes back byte-identical.
    let ops = m.dir.path().join("again.jsonl");
    std::fs::write(&ops, "{\"op\":\"view.add\",\"view\":\"V\",\"target\":\"Svc\"}\n").unwrap();
    let before = m.text();
    let (code, out, _) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(out.contains("false"), "reports the skip: {out}");
    assert_eq!(m.text(), before);
    assert_eq!(nodes(), 2);
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// `export views` wrote no `view.doc`, and `view.create` with `replace`
/// wiped the old view's documentation and viewpoint — so the round trip
/// SKILL.md called byte-identical deleted the documentation of every
/// documented view it touched. Both halves: the export carries them, and a
/// replace that names neither keeps them.
#[test]
fn exported_views_carry_documentation_and_a_replace_keeps_it() {
    let m = Model::new("modelimporter_test.archimate");
    let model_file = m.dir.path().join("m.archimate");
    for stale in ["View 1", "View 2"] {
        assert_eq!(m.run(&["view", "delete", stale, "-y"]).0, 0);
    }
    let seeded = |args: &[&str]| -> (i32, String, String) {
        let mut all = args.to_vec();
        all.extend_from_slice(&["--id-seed", "docs"]);
        m.run(&all)
    };
    assert_eq!(seeded(&["view", "create", "V", "--viewpoint", "layered"]).0, 0);
    assert_eq!(seeded(&["view", "add", "V", "BA1"]).0, 0);
    assert_eq!(seeded(&["view", "add", "V", "BR1"]).0, 0);
    assert_eq!(seeded(&["view", "doc", "V", "What V is for."]).0, 0);
    // Laid out, as a view that is kept is: the export re-lays every view it
    // rebuilds, so the round trip is exact only from a laid-out drawing.
    assert_eq!(seeded(&["view", "layout", "V", "--relayout-all"]).0, 0);
    let said = |m: &Model| -> String {
        let (_, out, _) = m.run(&["view", "list", "--fields", "name,doc,viewpoint", "-q"]);
        out.lines().find(|l| l.starts_with("V\t")).unwrap_or_default().to_string()
    };
    assert_eq!(said(&m), "V\tlayered\tWhat V is for.");

    // Where Archi writes it: after the objects, before any property. Archi's
    // next save would move a line written anywhere else.
    let text = m.text();
    let doc_at = text.find("<documentation>What V is for.").unwrap();
    let last_child = text.rfind("<child ").unwrap();
    assert!(doc_at > last_child, "documentation before the objects:\n{text}");

    let spec = m.dir.path().join("views.jsonl");
    assert_eq!(m.run(&["export", "views", "-o", spec.to_str().unwrap()]).0, 0);
    let batch = std::fs::read_to_string(&spec).unwrap();
    assert!(batch.contains(r#""op":"view.doc","view":"V","text":"What V is for.""#), "{batch}");
    assert!(batch.contains(r#""viewpoint":"layered""#), "{batch}");

    let before = std::fs::read_to_string(&model_file).unwrap();
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    let after = std::fs::read_to_string(&model_file).unwrap();
    assert_eq!(first_difference(&before, &after), None, "the round trip changed the file");
    assert_eq!(said(&m), "V\tlayered\tWhat V is for.");

    // A replace at the prompt that names no viewpoint keeps the old one and
    // the documentation; an explicit empty viewpoint clears it.
    assert_eq!(seeded(&["view", "create", "V", "--replace"]).0, 0);
    assert_eq!(said(&m), "V\tlayered\tWhat V is for.");
    assert_eq!(
        seeded(&["view", "auto", "V", "--from", "BA1", "--replace", "--viewpoint", ""]).0,
        0
    );
    assert_eq!(said(&m), "V\t\tWhat V is for.");
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// A batch `relation.add` with a `name` was accepted, reported success and
/// wrote a relationship without one — `--dry-run` said the same. A field an
/// operation does not take is refused now, and a name is a field it takes.
#[test]
fn a_batch_refuses_a_field_the_operation_does_not_take() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["element", "add", "BusinessActor", "A"]).0, 0);
    assert_eq!(m.run(&["element", "add", "Node", "N"]).0, 0);

    let ops = m.dir.path().join("named.jsonl");
    std::fs::write(
        &ops,
        r#"{"op":"relation.add","type":"Association","source":"A","target":"N","name":"owns"}"#,
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    let (_, out, _) = m.run(&["query", "type=Association", "--fields", "name,source_name", "-q"]);
    assert_eq!(out.trim(), "owns\tA");
    // In Archi's attribute order, so the line reads as Archi would write it.
    assert!(m.text().contains(r#"AssociationRelationship" name="owns" id="#), "{}", m.text());

    // And at the prompt.
    assert_eq!(m.run(&["relation", "add", "Association", "N", "A", "--name", "serves"]).0, 0);
    let (_, out, _) = m.run(&["query", "name=serves", "--fields", "type", "-q"]);
    assert_eq!(out.trim(), "AssociationRelationship");

    // A misspelt field is exit 2 naming the line, and nothing is written.
    let before = m.text();
    let bad = m.dir.path().join("bad.jsonl");
    std::fs::write(
        &bad,
        concat!(
            r#"{"op":"element.add","type":"Goal","name":"G"}"#,
            "\n",
            r#"{"op":"relation.add","type":"Association","source":"A","target":"N","nam":"x"}"#,
            "\n",
        ),
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", bad.to_str().unwrap(), "--dry-run"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("line 2") && err.contains("unknown field `nam`"), "{err}");
    assert!(err.contains("batch.md"), "the hint says where the fields are listed: {err}");
    let (code, _, _) = m.run(&["apply", bad.to_str().unwrap()]);
    assert_eq!(code, 2);
    assert_eq!(m.text(), before, "the line that parsed was not written either");
}

/// `id:` needed the id exactly as stored. The examples write `id:5dde26f7`,
/// the model's ids are `id-` and thirty-two hex characters, and a miss sent
/// the reader to `amcli search` — for an id.
#[test]
fn an_id_resolves_without_its_prefix_and_by_a_unique_prefix() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["element", "add", "Goal", "Ledger"]).0, 0);
    let (_, out, _) = m.run(&["query", "name=Ledger", "--fields", "id", "-q"]);
    let id = out.trim().to_string();
    let hex = id.strip_prefix("id-").expect("a new id has Archi's prefix");

    for sel in [id.clone(), hex.to_string(), hex[..8].to_string(), format!("id-{}", &hex[..8])] {
        let (code, out, err) = m.run(&["get", &format!("id:{sel}"), "--fields", "name", "-q"]);
        assert_eq!(code, 0, "id:{sel}: {err}");
        assert_eq!(out.trim(), "Ledger", "id:{sel}");
    }

    // Writes resolve the same way, and so do views.
    assert_eq!(m.run(&["element", "rename", &format!("id:{}", &hex[..8]), "Book"]).0, 0);
    assert_eq!(m.run(&["view", "create", "V"]).0, 0);
    let (_, out, _) = m.run(&["view", "list", "--fields", "id,name", "-q"]);
    let view_id = out.lines().find(|l| l.ends_with("\tV")).unwrap().split('\t').next().unwrap();
    let short = &view_id.strip_prefix("id-").unwrap()[..8];
    assert_eq!(m.run(&["view", "render", &format!("id:{short}"), "--as", "json"]).0, 0);

    // Too short to be a prefix, or nothing at all: a miss says what ids look
    // like here and what `id:` takes, not "try search".
    for sel in [&hex[..3], "ffffffff"] {
        let (code, _, err) = m.run(&["get", &format!("id:{sel}")]);
        assert_eq!(code, 3, "{err}");
        // The sample is one of this model's own ids, whatever style it uses.
        assert!(err.contains("ids in this model look like `"), "{err}");
        assert!(err.contains("prefix"), "{err}");
        assert!(!err.contains("amcli search"), "{err}");
    }
}

/// `query -F json` rows carry no `properties` key — jq prints `null` for a
/// missing key, which read as a null value — while `get` carries the list.
/// The list is there on request, as the same array, and one property is a
/// column.
#[test]
fn a_list_row_carries_properties_only_when_asked() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["element", "add", "Goal", "Ledger"]).0, 0);
    assert_eq!(m.run(&["prop", "set", "Ledger", "owner", "team-a"]).0, 0);
    let json = |args: &[&str]| -> serde_json::Value {
        let (_, out, _) = m.run(args);
        serde_json::from_str(&out).unwrap()
    };

    let row = json(&["query", "name=Ledger", "-F", "json"]);
    assert!(row["data"][0].get("properties").is_none(), "absent, not null: {row}");

    let expected = serde_json::json!([{"key": "owner", "value": "team-a"}]);
    let got = json(&["get", "Ledger", "-F", "json"]);
    assert_eq!(got["data"][0]["properties"], expected);
    let got = json(&["query", "name=Ledger", "-F", "json", "--fields", "name,properties"]);
    assert_eq!(got["data"][0]["properties"], expected, "the same shape on request: {got}");

    // In text a list is a count, and one property is its value.
    let (_, out, _) =
        m.run(&["query", "name=Ledger", "--fields", "name,properties,prop:owner", "-q"]);
    assert_eq!(out.trim(), "Ledger\t1\tteam-a");
}

/// `deg` filtered (`deg>10`) but did not print: `--fields name,deg` was a
/// note after the rows, and nothing at all under `-q`. Now it prints, and a
/// field no record has is refused before a row is — on a read. A write has
/// already landed by then, so there it is a warning ahead of the row: an
/// error would read as "the write failed", and the retry would add twice.
#[test]
fn a_field_no_record_has_is_refused_before_any_row() {
    let m = Model::new("modelimporter_test.archimate");

    let (code, out, err) =
        m.run(&["query", "kind=element and deg>0", "--fields", "name,deg", "-q"]);
    assert_eq!(code, 0, "{err}");
    let r = rows(&out);
    assert!(!r.is_empty());
    for row in &r {
        assert_eq!(row.len(), 2, "{row:?}");
        assert!(row[1].parse::<u32>().unwrap() > 0, "deg is in + out: {row:?}");
    }
    let (_, out, _) = m.run(&["query", "name=BA1", "--fields", "in,out,deg", "-q"]);
    let r = rows(&out).remove(0);
    let (i, o, d) =
        (r[0].parse::<u32>().unwrap(), r[1].parse::<u32>().unwrap(), r[2].parse::<u32>().unwrap());
    assert_eq!(d, i + o);

    // A misspelling is exit 2 with nothing on stdout, `-q` or not.
    for quiet in [&["-q"][..], &[][..]] {
        let mut args = vec!["query", "kind=element", "--fields", "name,dge"];
        args.extend_from_slice(quiet);
        let (code, out, err) = m.run(&args);
        assert_eq!(code, 2, "{err}");
        assert!(out.is_empty(), "rows were printed under a wrong projection: {out}");
        assert!(err.contains("no such field: dge"), "{err}");
        assert!(err.contains("deg"), "the columns it does have are named: {err}");
    }

    // On a write the element exists, the exit is 0, and the warning is said
    // whatever the flags — before the row.
    let (code, out, err) = m.run(&["element", "add", "Goal", "Landed", "--fields", "bogus", "-q"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("no such field: bogus"), "{err}");
    assert!(out.trim().is_empty(), "nothing matched the projection: {out}");
    let (_, out, _) = m.run(&["query", "name=Landed", "--count"]);
    assert_eq!(out.trim(), "1", "the write landed");
}

/// `get` listed a relationship as `other_id`/`other_name`, `query
/// 'kind=relation'` as `source`/`target` — two shapes for one thing, so no
/// one `jq` filter read both. `get` keeps `direction` and `other_*` and
/// carries the ends too.
#[test]
fn a_relationship_reads_the_same_wherever_it_turns_up() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["relation", "add", "Association", "BA1", "BR1", "--name", "owns"]).0, 0);
    let json = |args: &[&str]| -> serde_json::Value {
        let (_, out, _) = m.run(args);
        serde_json::from_str(&out).unwrap()
    };
    let nested = json(&["get", "BA1", "-F", "json"])["data"][0]["relations"].clone();
    let owns = nested.as_array().unwrap().iter().find(|r| r["name"] == "owns").unwrap();
    for key in [
        "id",
        "direction",
        "type",
        "other_id",
        "other_name",
        "source",
        "source_name",
        "target",
        "target_name",
    ] {
        assert!(owns.get(key).is_some(), "get's relation lacks `{key}`: {owns}");
    }

    let listed = json(&["query", "name=owns", "-F", "json"])["data"][0].clone();
    for key in ["id", "type", "name", "source", "source_name", "target", "target_name"] {
        assert_eq!(owns[key], listed[key], "`{key}` differs between get and query");
    }
    assert_eq!(owns["direction"], "out");
    assert_eq!(owns["source_name"], "BA1");
    assert_eq!(owns["target_name"], "BR1");
}

/// Ten scratch copies of a model beside the working directory made every
/// `apply` an exit 4. Still an exit 4 — a dry run that silently picked the
/// real model beside the batch would be a real run — but the hint now names
/// `AMCLI_MODEL`, and the one model the batch file sits beside.
#[test]
fn an_ambiguous_discovery_names_the_model_beside_the_batch() {
    let m = Model::new("modelimporter_test.archimate");
    let scratch = m.dir.path().join("scratch");
    let real = m.dir.path().join("real");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::create_dir_all(&real).unwrap();
    for copy in ["a", "b"] {
        std::fs::copy(m.path(), scratch.join(format!("{copy}.archimate"))).unwrap();
    }
    std::fs::copy(m.path(), real.join("one.archimate")).unwrap();
    let batch = real.join("fix.jsonl");
    std::fs::write(&batch, "{\"op\":\"element.add\",\"type\":\"Goal\",\"name\":\"Z\"}\n").unwrap();

    let out = Command::cargo_bin("amcli")
        .unwrap()
        .current_dir(&scratch)
        .args(["apply", "../real/fix.jsonl", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("2 models in"), "{err}");
    assert!(err.contains("AMCLI_MODEL"), "{err}");
    assert!(err.contains("one.archimate"), "the model beside the batch is named: {err}");
    assert!(err.contains("pass -m"), "{err}");

    // Named explicitly, the same command runs.
    let out = Command::cargo_bin("amcli")
        .unwrap()
        .current_dir(&scratch)
        .args(["-m", "../real/one.archimate", "apply", "../real/fix.jsonl", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

/// Renaming a folder used to take three commands — make the new one, move
/// every view, delete the old one — which gave the folder a new id and moved
/// every view's bytes: a four-thousand-line diff for one word. `folder rename`
/// changes one attribute, and `folder move` re-files a folder whole.
#[test]
fn a_folder_is_renamed_in_place_and_moved_with_its_contents() {
    let m = Model::new("modelimporter_test.archimate");
    assert_eq!(m.run(&["folder", "add", "/Views", "Alpha"]).0, 0);
    assert_eq!(m.run(&["folder", "add", "/Views/Alpha", "Inner"]).0, 0);
    assert_eq!(m.run(&["view", "create", "V", "-f", "/Views/Alpha/Inner"]).0, 0);
    assert_eq!(m.run(&["view", "add", "V", "BA1"]).0, 0);
    let id_of = |path: &str| -> String {
        let (_, out, _) = m.run(&["folder", "list", "-q"]);
        rows(&out).iter().find(|r| r[0] == path).map(|r| r[2].to_string()).unwrap_or_default()
    };
    let id = id_of("/Views/Alpha/Inner");
    assert!(!id.is_empty());

    let before = m.text();
    let (code, out, err) = m.run(&["folder", "rename", "/Views/Alpha/Inner", "Beta"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("/Views/Alpha/Beta"), "{out}");
    assert_eq!(id_of("/Views/Alpha/Beta"), id, "the id survives a rename");
    let after = m.text();
    let changed: Vec<&str> = after.lines().filter(|l| !before.lines().any(|b| b == *l)).collect();
    assert_eq!(changed.len(), 1, "one line changes: the folder's own: {changed:?}");
    assert!(changed[0].contains(r#"name="Beta""#), "{changed:?}");
    let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name,folder"]);
    assert!(out.contains("/Views/Alpha/Beta"), "the view is filed under the new name: {out}");

    // Moved under the views root, with the view inside it.
    let (code, _, err) = m.run(&["folder", "move", "/Views/Alpha/Beta", "--parent", "/Views"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(id_of("/Views/Beta"), id);
    let (_, out, _) = m.run(&["view", "list", "-q", "--fields", "name,folder"]);
    assert!(out.contains("V\t/Views/Beta"), "{out}");

    // Never out of its tree, never the top folders, never into itself.
    let (code, _, err) = m.run(&["folder", "move", "/Views/Beta", "--parent", "/Business"]);
    assert_eq!(code, 5, "{err}");
    assert!(err.contains("stays inside the top-level folder"), "{err}");
    let (code, _, err) = m.run(&["folder", "rename", "/Views", "Drawings"]);
    assert_eq!(code, 5, "{err}");
    assert_eq!(m.run(&["folder", "add", "/Views/Beta", "Deep"]).0, 0);
    let (code, _, err) = m.run(&["folder", "move", "/Views/Beta", "--parent", "/Views/Beta/Deep"]);
    assert_eq!(code, 5, "{err}");
    assert!(err.contains("inside it"), "{err}");

    // And in a batch.
    let ops = m.dir.path().join("f.jsonl");
    std::fs::write(
        &ops,
        "{\"op\":\"folder.rename\",\"path\":\"/Views/Beta\",\"name\":\"Gamma\"}\n\
         {\"op\":\"folder.move\",\"path\":\"/Views/Gamma/Deep\",\"parent\":\"/Views\"}\n",
    )
    .unwrap();
    let (code, _, err) = m.run(&["apply", ops.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(id_of("/Views/Gamma"), id);
    assert!(!id_of("/Views/Deep").is_empty());
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// A box drawn inside another box is how Archi shows the relationship
/// between them, and it draws no line for it. So `view add --into` nests,
/// the relationship counts as on the view, nothing is drawn between the two,
/// and `view nest` does what dragging a box into another does in Archi.
#[test]
fn a_box_nested_in_another_stands_for_the_relationship_between_them() {
    let m = Model::new("modelimporter_test.archimate");
    for n in ["Outer", "Inner", "Third"] {
        assert_eq!(m.run(&["element", "add", "ApplicationComponent", n]).0, 0);
    }
    assert_eq!(m.run(&["relation", "add", "Composition", "Outer", "Inner"]).0, 0);
    assert_eq!(m.run(&["relation", "add", "Serving", "Outer", "Third"]).0, 0);
    assert_eq!(m.run(&["view", "create", "V"]).0, 0);
    assert_eq!(m.run(&["view", "add", "V", "Outer"]).0, 0);
    let (code, out, err) = m.run(&["view", "add", "V", "Inner", "--into", "Outer"]);
    assert_eq!(code, 0, "{err}");
    let row = rows(&out).remove(0);
    assert!(row.last().unwrap().starts_with("id-"), "the row names the container: {out}");
    assert_eq!(row[4], "0", "no line is drawn to the container: {out}");
    assert_eq!(m.run(&["view", "add", "V", "Third"]).0, 0);

    // The file nests the box and keeps Archi's order inside an object:
    // bounds, the lines leaving it, then what it holds.
    let text = m.text();
    let view = text.find(r#"name="V""#).unwrap();
    let block = &text[view..];
    let outer = block.find(r#"<child xsi:type="archimate:DiagramObject""#).unwrap();
    let block = &block[outer..];
    let bounds = block.find("<bounds").unwrap();
    let conn = block.find("<sourceConnection").unwrap();
    let inner = 1 + block[1..].find(r#"<child xsi:type="archimate:DiagramObject""#).unwrap();
    let close = block.find("</child>").unwrap();
    assert!(bounds < conn && conn < inner && inner < close, "bounds, connection, nested child");
    // Whole lines, from the view on: the block above starts mid-line.
    let mut object_lines =
        text[view..].lines().filter(|l| l.contains("archimateElement") && l.contains("<child"));
    let outer_line = object_lines.next().unwrap();
    let inner_line = object_lines.next().unwrap();
    let indent = |l: &str| l.len() - l.trim_start().len();
    assert_eq!(indent(inner_line), indent(outer_line) + 2, "nested one level deeper");

    // On the view, by nesting: nothing is drawn nowhere.
    let (_, out, _) = m.run(&["query", "kind=relation and views=0", "--count", "-q"]);
    assert_eq!(out.trim(), "0", "{out}");
    let (_, out, _) = m.run(&["view", "render", "V", "--as", "json", "-q"]);
    assert_eq!(out.matches(r#""concept":"#).count(), 3);
    assert_eq!(out.matches(r#""relationship":"#).count(), 1, "one line: Outer serves Third");

    // A relationship added later between the two is not drawn either — the
    // nesting already says it — and still counts as on the view.
    let (code, out, err) = m.run(&["relation", "add", "Flow", "Inner", "Outer"]);
    assert_eq!(code, 0, "{err}");
    let _ = out;
    let (_, out, _) = m.run(&["query", "kind=relation and views=0", "--count", "-q"]);
    assert_eq!(out.trim(), "0", "{out}");
    let (_, out, _) = m.run(&["get", "Inner", "-F", "json", "-q"]);
    assert!(out.contains(r#""views":[{"#), "get lists the view it is nested on: {out}");

    // Un-nested, the lines come back on `sync`; nested again, they go.
    assert_eq!(m.run(&["view", "nest", "V", "Inner"]).0, 0);
    let (code, out, err) = m.run(&["view", "sync", "V"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("\t2\t") || rows(&out)[0][2] == "2", "two lines drawn: {out}");
    let (code, out, err) = m.run(&["view", "nest", "V", "Inner", "--into", "Outer"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(rows(&out)[0][2], "2", "both lines removed: {out}");
    assert!(err.contains("nesting now stands for them"), "{err}");
    let (_, out, _) = m.run(&["view", "render", "V", "--as", "json", "-q"]);
    assert_eq!(out.matches(r#""relationship":"#).count(), 1);
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

/// Groups, notes and nesting are exported as the batch that rebuilds them,
/// and the rebuild is byte-identical — so a nested drawing amcli made can be
/// reviewed and regenerated like a flat one.
#[test]
fn nested_views_with_groups_and_notes_round_trip_through_export_views() {
    let m = Model::new("modelimporter_test.archimate");
    for stale in ["View 1", "View 2"] {
        assert_eq!(m.run(&["view", "delete", stale, "-y"]).0, 0);
    }
    let seed = ["--id-seed", "nested"];
    let seeded = |args: &[&str]| -> (i32, String, String) {
        let mut all = args.to_vec();
        all.extend_from_slice(&seed);
        m.run(&all)
    };
    for n in ["Outer", "Inner", "Aside"] {
        assert_eq!(seeded(&["element", "add", "ApplicationComponent", n]).0, 0);
    }
    assert_eq!(seeded(&["relation", "add", "Composition", "Outer", "Inner"]).0, 0);
    assert_eq!(seeded(&["relation", "add", "Serving", "Aside", "Inner"]).0, 0);
    assert_eq!(seeded(&["view", "create", "N"]).0, 0);
    let (code, out, err) = seeded(&["view", "group", "N", "Zone"]);
    assert_eq!(code, 0, "{err}");
    let zone = rows(&out)[0][0].to_string();
    assert_eq!(seeded(&["view", "add", "N", "Outer", "--into", "Zone"]).0, 0);
    assert_eq!(seeded(&["view", "add", "N", "Inner", "--into", "Outer"]).0, 0);
    assert_eq!(seeded(&["view", "add", "N", "Aside"]).0, 0);
    let (code, _, err) = seeded(&["view", "note", "N", "Read me first", "--into", &zone]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(seeded(&["view", "layout", "N", "--relayout-all"]).0, 0);

    // Laid out, every box sits inside what holds it.
    let (_, out, _) = m.run(&["view", "render", "N", "--as", "json", "-q"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let nodes = json["nodes"].as_array().unwrap();
    let rect = |label: &str| -> (i64, i64, i64, i64) {
        let n = nodes.iter().find(|n| n["label"] == label).unwrap_or_else(|| panic!("{label}"));
        (
            n["x"].as_i64().unwrap(),
            n["y"].as_i64().unwrap(),
            n["w"].as_i64().unwrap(),
            n["h"].as_i64().unwrap(),
        )
    };
    let inside = |(x, y, w, h): (i64, i64, i64, i64), (px, py, pw, ph): (i64, i64, i64, i64)| {
        x >= px && y >= py && x + w <= px + pw && y + h <= py + ph
    };
    assert!(inside(rect("Outer"), rect("Zone")), "Outer in Zone: {out}");
    assert!(inside(rect("Inner"), rect("Outer")), "Inner in Outer: {out}");
    assert!(inside(rect("Read me first"), rect("Zone")), "the note in Zone: {out}");
    assert_eq!(
        json["edges"].as_array().unwrap().len(),
        1,
        "Aside serves Inner; the composition is the nesting"
    );

    let spec = m.dir.path().join("views.jsonl");
    assert_eq!(m.run(&["export", "views", "-o", spec.to_str().unwrap()]).0, 0);
    let text = std::fs::read_to_string(&spec).unwrap();
    assert!(text.contains(r#""op":"view.group","view":"N","name":"Zone","ref":"o1""#), "{text}");
    assert!(text.contains(r#""target":"Outer","into":"ref:o1""#), "{text}");
    assert!(text.contains(r#""target":"Inner","into":"Outer""#), "{text}");
    assert!(
        text.contains(r#""op":"view.note","view":"N","text":"Read me first","into":"ref:o1""#),
        "{text}"
    );
    assert!(!text.contains("not rebuilt"), "everything on the view is rebuilt: {text}");

    let before = m.text();
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    assert_eq!(first_difference(&before, &m.text()), None, "one round trip changes nothing");
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    assert_eq!(first_difference(&before, &m.text()), None, "and neither does a second");
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}

// ---- diff and merge ---------------------------------------------------------

/// Three files the binary itself built under one seed: BASE, and OURS and
/// THEIRS each a copy of it with a different batch applied. OURS adds an
/// element and renames another; THEIRS adds a different element and a view,
/// and changes a third element's documentation.
struct ThreeWay {
    dir: tempfile::TempDir,
}

impl ThreeWay {
    fn build() -> ThreeWay {
        let t = ThreeWay { dir: tempfile::tempdir().unwrap() };
        let (base, ours, theirs) = (t.file("base"), t.file("ours"), t.file("theirs"));
        assert_eq!(t.run(&["init", "Base", "-o", &base]).0, 0);
        t.apply(
            &base,
            concat!(
                r#"{"op":"element.add","type":"ApplicationComponent","name":"Alpha","folder":"/Application","ref":"a"}"#,
                "\n",
                r#"{"op":"element.add","type":"BusinessActor","name":"Beta","folder":"/Business","ref":"b"}"#,
                "\n",
                r#"{"op":"element.add","type":"DataObject","name":"Gamma","folder":"/Application","doc":"old doc"}"#,
                "\n",
                r#"{"op":"relation.add","type":"Serving","source":"ref:a","target":"ref:b"}"#,
                "\n",
                r#"{"op":"folder.add","parent":"/Views","name":"Group"}"#,
                "\n",
            ),
        );
        std::fs::copy(&base, &ours).unwrap();
        std::fs::copy(&base, &theirs).unwrap();
        t.apply(
            &ours,
            concat!(
                r#"{"op":"element.add","type":"ApplicationComponent","name":"Ours Added","folder":"/Application"}"#,
                "\n",
                r#"{"op":"element.rename","target":"Beta","name":"Beta renamed"}"#,
                "\n",
            ),
        );
        t.apply(
            &theirs,
            concat!(
                r#"{"op":"element.add","type":"BusinessRole","name":"Theirs Added","folder":"/Business"}"#,
                "\n",
                r#"{"op":"view.create","name":"V","folder":"/Views/Group"}"#,
                "\n",
                r#"{"op":"view.add","view":"V","target":"Alpha"}"#,
                "\n",
                r#"{"op":"view.add","view":"V","target":"Beta"}"#,
                "\n",
                r#"{"op":"element.doc","target":"Gamma","text":"new doc"}"#,
                "\n",
            ),
        );
        t
    }

    fn file(&self, name: &str) -> String {
        self.dir.path().join(format!("{name}.archimate")).to_str().unwrap().to_string()
    }

    /// The binary, seeded, with no `-m`: `diff` and `merge` name their files.
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::cargo_bin("amcli")
            .unwrap()
            .env("AMCLI_ID_SEED", "three-way")
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn apply(&self, model: &str, batch: &str) {
        let path = self.dir.path().join("batch.jsonl");
        std::fs::write(&path, batch).unwrap();
        let (code, _, err) = self.run(&["-m", model, "apply", path.to_str().unwrap()]);
        assert_eq!(code, 0, "{err}");
    }

    fn text(&self, name: &str) -> String {
        std::fs::read_to_string(self.file(name)).unwrap()
    }
}

#[test]
fn merge_reconciles_two_edits_of_one_model() {
    let t = ThreeWay::build();
    let (base, ours, theirs, result) =
        (t.file("base"), t.file("ours"), t.file("theirs"), t.file("result"));

    // A dry run says what it would do and writes nothing.
    let (code, out, _) = t.run(&["merge", &base, &ours, &theirs, "-o", &result, "--dry-run"]);
    assert_eq!(code, 0, "{out}");
    assert_eq!(rows(&out).len(), 3, "{out}");
    assert!(!Path::new(&result).exists(), "a dry run writes nothing");

    let (code, out, err) = t.run(&["merge", &base, &ours, &theirs, "-o", &result]);
    assert_eq!(code, 0, "{err}");
    let actions: Vec<(&str, &str)> = rows(&out).iter().map(|r| (r[2], r[3])).collect::<Vec<_>>();
    assert_eq!(
        actions,
        vec![("Gamma", "replaced"), ("Theirs Added", "inserted"), ("V", "inserted")],
        "{out}"
    );

    // The result is a model, and holds both sides' work.
    let (code, _, err) = t.run(&["-m", &result, "validate"]);
    assert_eq!(code, 0, "{err}");
    let merged = t.text("result");
    for wanted in ["Ours Added", "Theirs Added", "Beta renamed", "new doc", "name=\"V\""] {
        assert!(merged.contains(wanted), "missing {wanted}:\n{merged}");
    }
    assert!(!merged.contains("old doc"));
    // Ours' bytes are kept, not re-serialised: every line ours had is there
    // verbatim except the replaced block's own and the views folder that
    // was `<folder …/>` and now holds a view.
    let ours_text = t.text("ours");
    let replaced = |l: &str| l.contains("Gamma") || l.contains("old doc") || l.contains("Group");
    for line in ours_text.lines().filter(|l| !replaced(l)) {
        assert!(merged.contains(line), "ours' line was rewritten: {line}");
    }

    // Merging again with the result as ours is a no-op, byte for byte.
    std::fs::copy(&result, t.file("again")).unwrap();
    let (code, out, _) = t.run(&["merge", &base, &t.file("again"), &theirs]);
    assert_eq!(code, 0);
    assert_eq!(rows(&out).len(), 0, "{out}");
    assert_eq!(t.text("again"), merged);

    // Theirs identical to base leaves ours untouched, byte for byte.
    let (code, out, _) = t.run(&["merge", &base, &ours, &base, "-o", &t.file("same")]);
    assert_eq!(code, 0);
    assert_eq!(rows(&out).len(), 0, "{out}");
    assert_eq!(t.text("same"), ours_text);

    // Without -o the result goes over ours — the git merge driver's `%A`.
    std::fs::copy(&ours, t.file("inplace")).unwrap();
    let (code, _, err) = t.run(&["merge", &base, &t.file("inplace"), &theirs]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(t.text("inplace"), merged);
}

#[test]
fn merge_reports_a_change_on_both_sides_as_a_conflict_and_prefer_settles_it() {
    let t = ThreeWay::build();
    let (base, ours, theirs, result) =
        (t.file("base"), t.file("ours"), t.file("theirs"), t.file("result"));
    t.apply(&ours, "{\"op\":\"element.doc\",\"target\":\"Gamma\",\"text\":\"ours doc\"}\n");
    let ours_before = t.text("ours");

    let (code, out, err) = t.run(&["merge", &base, &ours, &theirs, "-o", &result]);
    assert_eq!(code, 6, "conflict: {err}");
    let conflicts = rows(&out);
    assert_eq!(conflicts.len(), 1, "{out}");
    assert_eq!(conflicts[0][0], "element");
    assert_eq!(conflicts[0][2], "Gamma");
    assert!(conflicts[0][3].contains("both sides"), "{out}");
    assert!(err.contains("--prefer"), "{err}");
    assert!(!Path::new(&result).exists(), "a conflicted merge writes nothing");

    // In place it is the same refusal, and ours is untouched.
    let (code, _, _) = t.run(&["merge", &base, &ours, &theirs]);
    assert_eq!(code, 6);
    assert_eq!(t.text("ours"), ours_before);

    // A side settles it.
    let (code, out, err) =
        t.run(&["merge", &base, &ours, &theirs, "-o", &result, "--prefer", "theirs"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("replaced"), "{out}");
    let merged = t.text("result");
    assert!(merged.contains("new doc") && !merged.contains("ours doc"), "{merged}");
    assert!(merged.contains("Theirs Added") && merged.contains("Ours Added"));

    let (code, _, err) =
        t.run(&["merge", &base, &ours, &theirs, "-o", &result, "--prefer", "ours"]);
    assert_eq!(code, 0, "{err}");
    let merged = t.text("result");
    assert!(merged.contains("ours doc") && !merged.contains("new doc"), "{merged}");
    assert!(merged.contains("Theirs Added"), "the rest of theirs still comes across");

    let (code, _, err) = t.run(&["merge", &base, &ours, &theirs, "--prefer", "mine"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn diff_lists_exactly_the_changes_and_ignores_serialisation_noise() {
    let t = ThreeWay::build();
    let (base, theirs) = (t.file("base"), t.file("theirs"));

    let (code, out, err) = t.run(&["diff", &base, &theirs]);
    assert_eq!(code, 0, "{err}");
    let mut found: Vec<(&str, &str, &str, &str)> =
        rows(&out).iter().map(|r| (r[0], r[1], r[3], r[4])).collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            ("added", "element", "Theirs Added", "/Business"),
            ("added", "view", "V", "/Views/Group"),
            ("changed", "element", "Gamma", "documentation"),
        ],
        "{out}"
    );
    let (_, json, _) = t.run(&["diff", &base, &theirs, "-F", "json"]);
    assert!(json.contains(r#""differences":3"#), "{json}");

    let (_, out, _) = t.run(&["diff", &base, &t.file("ours")]);
    let mut found: Vec<(&str, &str)> = rows(&out).iter().map(|r| (r[0], r[4])).collect();
    found.sort();
    assert_eq!(
        found,
        vec![("added", "/Application"), ("renamed", "name Beta → Beta renamed")],
        "{out}"
    );

    // Another tool's save of the same model — attributes shuffled on one
    // element, a default written out on the view, `>` spelled as an entity —
    // is not a difference.
    let original = t.text("theirs");
    let shuffled = original
        .replacen(
            r#"<element xsi:type="archimate:DataObject" name="Gamma""#,
            r#"<element name="Gamma" xsi:type="archimate:DataObject""#,
            1,
        )
        .replacen(r#"<bounds x="0" y="0" "#, r#"<bounds "#, 1)
        .replacen("new doc", "new doc &gt; old", 1);
    assert_ne!(shuffled, original, "the copy is a different file");
    let noisy = t.file("noisy");
    std::fs::write(&noisy, shuffled).unwrap();
    let (code, out, _) = t.run(&["diff", &theirs, &noisy]);
    assert_eq!(code, 0);
    assert_eq!(rows(&out).len(), 1, "only the documentation really changed: {out}");
    assert_eq!(rows(&out)[0][4], "documentation");

    // With the same documentation spelled two ways: nothing at all.
    let same = original
        .replacen(
            r#"<element xsi:type="archimate:DataObject" name="Gamma""#,
            r#"<element name="Gamma" xsi:type="archimate:DataObject""#,
            1,
        )
        .replacen(r#"<bounds x="0" y="0" "#, r#"<bounds "#, 1);
    std::fs::write(&noisy, same).unwrap();
    let (code, out, _) = t.run(&["diff", &theirs, &noisy]);
    assert_eq!(code, 0);
    assert_eq!(out, "", "no difference: {out}");
    let (_, json, _) = t.run(&["diff", &theirs, &noisy, "-F", "json"]);
    assert!(json.contains(r#""differences":0"#), "{json}");
}

/// A poster — regions, captions, colours, one chosen line routed around
/// things — is built, styled and exported as a batch, and the batch
/// rebuilds it byte for byte.
#[test]
fn a_styled_view_is_exported_as_it_was_drawn_and_rebuilds_byte_for_byte() {
    let m = Model::new("modelimporter_test.archimate");
    for stale in ["View 1", "View 2"] {
        assert_eq!(m.run(&["view", "delete", stale, "-y"]).0, 0);
    }
    let seed = ["--id-seed", "poster"];
    let seeded = |args: &[&str]| -> (i32, String, String) {
        let mut all = args.to_vec();
        all.extend_from_slice(&seed);
        m.run(&all)
    };
    for n in ["Gateway", "Ledger"] {
        assert_eq!(seeded(&["element", "add", "ApplicationComponent", n]).0, 0);
    }
    assert_eq!(
        seeded(&["relation", "add", "Flow", "Gateway", "Ledger", "--name", "postings"]).0,
        0
    );
    assert_eq!(seeded(&["relation", "add", "Serving", "Ledger", "Gateway"]).0, 0);
    assert_eq!(seeded(&["view", "create", "P"]).0, 0);
    let (code, out, err) = seeded(&[
        "view", "group", "P", "Region", "--x", "0", "--y", "0", "--width", "600", "--height", "300",
    ]);
    assert_eq!(code, 0, "{err}");
    let region = rows(&out)[0][0].to_string();
    // Placed by hand, no connections drawn, one box twice.
    assert_eq!(
        seeded(&[
            "view",
            "add",
            "P",
            "Gateway",
            "--into",
            "Region",
            "--x",
            "20",
            "--y",
            "60",
            "--width",
            "200",
            "--height",
            "80",
            "--no-connect"
        ])
        .0,
        0
    );
    assert_eq!(
        seeded(&[
            "view",
            "add",
            "P",
            "Ledger",
            "--into",
            "Region",
            "--x",
            "360",
            "--y",
            "60",
            "--no-connect"
        ])
        .0,
        0
    );
    let (code, out, err) = seeded(&[
        "view",
        "add",
        "P",
        "Ledger",
        "--again",
        "--x",
        "700",
        "--y",
        "60",
        "--no-connect",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(rows(&out)[0][5], "true", "a second box was drawn on purpose: {out}");
    let (_, out, _) = m.run(&["view", "render", "P", "--as", "json", "-q"]);
    assert_eq!(out.matches(r#""concept":"id-"#).count(), 3);
    assert_eq!(out.matches(r#""relationship":"#).count(), 0, "nothing wired yet");

    // Styled as a person would in Archi.
    let (code, out, err) = seeded(&[
        "view",
        "style",
        "P",
        "Region",
        "--fill",
        "#EEF5FC",
        "--line",
        "#a4b4c5",
        "--line-width",
        "2",
        "--font-size",
        "20",
        "--font-style",
        "bold",
        "--font-color",
        "#183047",
        "--border",
        "rectangle",
        "--text-align",
        "left",
    ]);
    assert_eq!(code, 0, "{err}");
    assert!(rows(&out)[0][2].contains("fillColor") && rows(&out)[0][2].contains("font"), "{out}");
    assert_eq!(
        seeded(&[
            "view",
            "style",
            "P",
            "Gateway",
            "--label",
            "${name}\nedge of the cell",
            "--icon",
            "hide",
            "--text-position",
            "top"
        ])
        .0,
        0
    );
    let (code, _, err) = seeded(&["view", "style", "P", "Region", "--fill", "not-a-colour"]);
    assert_eq!(code, 2, "a bad colour is refused: {err}");

    // One chosen line, routed, styled — where sync would draw both.
    let (code, out, err) =
        seeded(&["view", "connect", "P", "Gateway", "Ledger", "--relationship", "Flow:postings"]);
    assert_eq!(code, 0, "{err}");
    let line = rows(&out)[0][0].to_string();
    assert_eq!(seeded(&["view", "route", "P", &line, "--points", "120,250", "460,250"]).0, 0);
    assert_eq!(
        seeded(&[
            "view",
            "style",
            "P",
            "rel:Flow:postings",
            "--line",
            "#2563a6",
            "--font-size",
            "14",
            "--font-style",
            "italic",
            "--label",
            "1 · postings"
        ])
        .0,
        0
    );
    let (_, svg, _) = m.run(&["view", "render", "P", "-q"]);
    assert!(svg.contains(r##"fill="#eef5fc""##) && svg.contains("font-weight=\"bold\""), "{svg}");
    assert!(svg.contains(">edge of the cell<") && svg.contains(">1 · postings<"), "{svg}");
    assert!(svg.contains("120,250 460,250"), "the route is drawn through the points: {svg}");
    let (_, out, _) = m.run(&["view", "render", "P", "--as", "json", "-q"]);
    assert_eq!(out.matches(r#""relationship":"#).count(), 1, "only the chosen line");

    // Exported as drawn: bounds, one connect, styles, a route, no layout.
    let spec = m.dir.path().join("poster.jsonl");
    assert_eq!(m.run(&["export", "views", "-o", spec.to_str().unwrap()]).0, 0);
    let text = std::fs::read_to_string(&spec).unwrap();
    assert!(text.contains(r#""op":"view.group","view":"P","name":"Region","ref":"o1","x":0,"y":0,"width":600,"height":300"#), "{text}");
    assert!(text.contains(r#""target":"Ledger","into":"ref:o1","x":360,"y":60"#), "{text}");
    assert!(text.contains(r#""again":true"#), "{text}");
    assert!(
        text.contains(r##""op":"view.style","view":"P","target":"ref:o1","fill":"#eef5fc""##),
        "{text}"
    );
    assert!(text.contains(r#""op":"view.connect","view":"P""#), "{text}");
    assert!(
        text.contains(r#""op":"view.route","view":"P","target":"ref:c"#)
            && text.contains("[[120,250],[460,250]]"),
        "{text}"
    );
    assert!(
        !text.contains(r#""op":"view.layout","view":"P""#),
        "no layout for a hand-drawn view: {text}"
    );
    let _ = region;

    let before = m.text();
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    assert_eq!(first_difference(&before, &m.text()), None, "one round trip changes nothing");
    assert_eq!(seeded(&["apply", spec.to_str().unwrap()]).0, 0);
    assert_eq!(first_difference(&before, &m.text()), None, "and neither does a second");
    assert_eq!(m.run(&["validate", "--level", "integrity"]).0, 0);
}
