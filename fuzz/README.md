# Fuzzing `iamb-core`

The `template` target parses, resolves and renders arbitrary templates against a small fixed
library (`fuzz_targets/template.rs`). Besides crashes, it checks that every error comes from the
phase that may find it and that successful output is well formed. This directory is its own
workspace and the only code built with nightly.

From the workspace root, seed the corpus with the template-spec examples, then fuzz from this
directory:

    python3 -I spec/template/seed_fuzz_corpus.py fuzz/corpus/template
    cd fuzz
    cargo +nightly fuzz run template corpus/template -- -timeout=1 -max_total_time=600 -rss_limit_mb=512

`-timeout=1` is the per-input time limit: any single input that takes more than one second is
reported as a failure, so slow inputs are caught as well as crashes (review 1.9 findings 1 and 9).
`-rss_limit_mb=512` does the same for memory. The nightly workflow runs exactly this command.
Failing inputs are written to `artifacts/template/`; replay one with
`cargo +nightly fuzz run template artifacts/template/<file>`.
