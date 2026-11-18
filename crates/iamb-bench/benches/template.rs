//! Template benchmarks (task 1.3): `template/render/5kb` and its parts.
//!
//! The fixture is a realistic 5 KB snippet (a support reply): prose with about twenty macros —
//! dates and times with shifts, text, choice and toggle fields, an `{if}`/`{else}`, a cursor and
//! two nested snippets (a signature that itself includes a footer with a date). The KPI
//! (`docs/test-strategy.md` §2.2: "5 KB snippet renders < 1 ms") is measured on the whole
//! pipeline an expansion runs — parse, resolve and render — with 1,000 individually timed runs;
//! the pass rule is p99 < 1 ms.
//!
//! Run: `cargo bench -p iamb-bench --bench template -- --quick`

use criterion::{Criterion, criterion_group};
use iamb_bench::{LatencyRecorder, ResultRecord, RunWriter};
use iamb_core::datetime::{Format, LocalDateTime, Locale};
use iamb_core::template::{FormValues, RenderContext, Rendered, Template};
use std::collections::HashMap;
use std::hint::black_box;

/// Individually timed runs per percentile record.
const RECORDED_RUNS: usize = 1_000;
/// KPI target: p99 under 1 ms.
const RENDER_TARGET_NS: u64 = 1_000_000;
/// Fixture size bounds, in bytes.
const FIXTURE_BYTES: std::ops::RangeInclusive<usize> = 5_000..=5_400;

const PARAGRAPH: &str = "Our team has reviewed the details you sent and the history of the \
account. Everything you described matches what we see on our side, and nothing further is \
needed from you at this stage. If anything changes before then, simply reply to this message \
and the same person will pick it up; there is no need to open a new request.\n\n";

/// The 5 KB template and the library it uses.
fn fixture() -> (String, HashMap<String, String>) {
    let mut t = String::new();
    t.push_str("Hello {field name=\"Customer\" default=\"there\"},\n\n");
    t.push_str("We appreciate your patience while we looked into this for you. ");
    t.push_str(
        "Thank you for contacting us on {date fmt=\"%A, %-d %B %Y\"} about order \
         {field name=\"Order\"}. ",
    );
    t.push_str(PARAGRAPH);
    t.push_str(
        "{if field=\"Plan\" is=\"Team\"}As a Team customer you have a named contact: \
         {field name=\"Agent\" default=\"the support desk\"}.{else}You can upgrade to the Team \
         plan at any time from the billing page.{endif}\n\n",
    );
    t.push_str(PARAGRAPH);
    t.push_str(
        "The replacement ships on {date fmt=\"%a %d %b\" shift=\"+2bd\"} and should arrive by \
         {date fmt=\"%A %-d %B\" shift=\"+5bd\"}. Your refund of {field name=\"Amount\"} will be \
         visible by {date shift=\"+1w +2d\"} at the latest. ",
    );
    t.push_str(PARAGRAPH);
    t.push_str(
        "Plan on file: {field name=\"Plan\" type=choice options=\"Free|Pro|Team\" default=Pro}. \
         Next invoice: {date fmt=\"%-d %b %Y\" shift=\"+1mo -1d\"}.\
         {field name=\"Rush\" type=toggle on=\" Express shipping is included.\" off=\"\"} ",
    );
    t.push_str(PARAGRAPH);
    t.push_str(
        "This reply was written at {time} ({time fmt=\"%-I:%M %p\" shift=\"+0min\"}); we will \
         check in again around {time shift=\"+26h\"} on {date shift=\"+1bd\" fmt=%A}.\n\n",
    );
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(PARAGRAPH);
    t.push_str(
        "Reference: {field name=\"Order\"}-{date fmt=%Y%m%d}-{time fmt=%H%M%S}. {cursor}\n\n",
    );
    t.push_str(PARAGRAPH);
    t.push_str("{snippet abbr=\";sig\"}");

    let mut lib = HashMap::new();
    lib.insert(
        ";sig".to_owned(),
        "Kind regards,\n{field name=\"Agent\" default=\"the support desk\"}\n\
         {snippet abbr=\";footer\"}"
            .to_owned(),
    );
    lib.insert(
        ";footer".to_owned(),
        "--\nSupport · open Monday to Friday · {date fmt=\"%B %Y\"}\n{lbrace}ref} is our \
         reference format."
            .to_owned(),
    );
    (t, lib)
}

struct Settings {
    date: Format,
    time: Format,
    values: FormValues,
}

impl Settings {
    fn new() -> Self {
        Self {
            date: Format::parse("%Y-%m-%d").expect("date format"),
            time: Format::parse("%H:%M").expect("time format"),
            values: [
                ("Customer", "Ana"),
                ("Order", "A-10442"),
                ("Plan", "Team"),
                ("Amount", "€42.50"),
                ("Rush", "yes"),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn ctx(&self) -> RenderContext<'_> {
        RenderContext {
            now: LocalDateTime::new(2026, 3, 18, 14, 5, 9).expect("now"),
            locale: Locale::from_tag("en"),
            date_format: &self.date,
            time_format: &self.time,
            clipboard: None,
        }
    }
}

/// The whole expansion pipeline, as the engine runs it.
fn expand(source: &str, lib: &HashMap<String, String>, settings: &Settings) -> Rendered {
    Template::parse(source)
        .and_then(|t| t.resolve(lib))
        .and_then(|r| r.render(&settings.ctx(), &settings.values))
        .expect("fixture renders")
}

fn bench_template(c: &mut Criterion) {
    let (source, lib) = fixture();
    let settings = Settings::new();

    c.bench_function("template/render/5kb", |b| {
        b.iter(|| expand(black_box(&source), black_box(&lib), &settings));
    });
    c.bench_function("template/parse/5kb", |b| {
        b.iter(|| Template::parse(black_box(&source)).expect("parses"));
    });
    let template = Template::parse(&source).expect("parses");
    c.bench_function("template/resolve/5kb", |b| {
        b.iter(|| black_box(&template).resolve(&lib).expect("resolves"));
    });
    let resolved = template.resolve(&lib).expect("resolves");
    c.bench_function("template/render_only/5kb", |b| {
        b.iter(|| {
            black_box(&resolved)
                .render(&settings.ctx(), &settings.values)
                .expect("renders")
        });
    });
}

criterion_group!(benches, bench_template);

/// Times each run individually and writes the percentile records to the results file.
fn record_percentiles() -> std::io::Result<()> {
    let (source, lib) = fixture();
    assert!(
        FIXTURE_BYTES.contains(&source.len()),
        "fixture is {} bytes",
        source.len()
    );
    let settings = Settings::new();
    let template = Template::parse(&source).expect("parses");
    let resolved = template.resolve(&lib).expect("resolves");
    let typed = expand(&source, &lib, &settings).text().len();

    // Warm up caches and the branch predictor before recording.
    for _ in 0..100 {
        black_box(expand(&source, &lib, &settings));
    }
    let mut full = LatencyRecorder::with_capacity(RECORDED_RUNS);
    let mut render_only = LatencyRecorder::with_capacity(RECORDED_RUNS);
    for _ in 0..RECORDED_RUNS {
        full.time(|| black_box(expand(black_box(&source), &lib, &settings)));
        render_only.time(|| {
            black_box(
                black_box(&resolved)
                    .render(&settings.ctx(), &settings.values)
                    .expect("renders"),
            )
        });
    }

    let note = format!(
        "{} B template, ~20 macros, 2 nested snippets, {typed} B typed; samples include \
         Instant overhead",
        source.len()
    );
    let mut writer = RunWriter::in_default_dir("template");
    let mut lines = Vec::new();
    for (bench, rec, kpi, what) in [
        (
            "template/render/5kb",
            &full,
            Some("template-render-p99"),
            "parse + resolve + render",
        ),
        (
            "template/render_only/5kb",
            &render_only,
            None,
            "render of a resolved template",
        ),
    ] {
        let Some(summary) = rec.summary() else {
            continue;
        };
        let mut record = ResultRecord::new(bench, summary);
        record.kpi = kpi.map(str::to_owned);
        record.target_stat = Some("p99");
        record.target_ns = Some(RENDER_TARGET_NS);
        record.note = Some(format!("{what}; {note}"));
        writer.push(&record);
        let samples = writer.write_samples(bench, &rec.sorted_samples())?;
        lines.push((record, samples));
    }
    let path = writer.finish()?;
    report(&lines, &path);
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "a benchmark reports its numbers on stdout"
)]
fn report(lines: &[(ResultRecord, std::path::PathBuf)], path: &std::path::Path) {
    for (r, samples) in lines {
        let s = &r.summary;
        let verdict = match (r.target_value(), r.target_ns) {
            (Some(v), Some(t)) if v < t => " [pass: p99 < target]",
            (Some(_), Some(_)) => " [FAIL: p99 >= target]",
            _ => "",
        };
        println!(
            "{} (individually timed, n={}): min {} ns, p50 {} ns, p95 {} ns, p99 {} ns, \
             max {} ns{verdict}",
            r.bench, s.count, s.min, s.p50, s.p95, s.p99, s.max
        );
        println!("  samples: {}", samples.display());
    }
    println!("results: {}", path.display());
}

fn main() {
    benches();
    Criterion::default().configure_from_args().final_summary();
    if let Err(e) = record_percentiles() {
        eprintln!("failed to write bench results: {e}");
        std::process::exit(1);
    }
}
