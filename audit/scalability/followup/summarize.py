import json
from pathlib import Path
import statistics

OUT = Path(__file__).resolve().parent
primary = json.loads((OUT / "measurements.json").read_text())
final = json.loads((OUT / "final-checks.json").read_text())
before = [r for r in final if r["label"].startswith("before-million-")]
after = [r for r in final if r["label"].startswith("final-million-")]
def med(rows, key):
    return statistics.median(r[key] for r in rows)
rows = []
for label, key, fmt in [("Updates/second", "updates_per_second", ",.0f"), ("CPU seconds / million updates", "cpu_seconds", ".2f"),
                        ("p50 latency (microseconds)", "p50_us", ".3f"), ("p95 latency (microseconds)", "p95_us", ".3f"),
                        ("p99 latency (microseconds)", "p99_us", ".3f"), ("Final RSS (KiB)", "rss_kib", ",.0f")]:
    rows.append(f"| {label} | {med(before, key):{fmt}} | {med(after, key):{fmt}} |")
stats_before = next(r for r in primary if r["label"] == "before-stats")
stats_after = next(r for r in final if r["label"] == "final-owned-stats")
soak = next(r for r in primary if r["label"] == "after-soak")
text = "\n### Final measured results\n\nMedian of three alternating one-million-update runs per version; identical warmed text workload:\n\n"
text += "| Metric | Before this follow-up | Final |\n|---|---:|---:|\n" + "\n".join(rows) + "\n\n"
text += f"Throughput changed by {(med(after, 'updates_per_second') / med(before, 'updates_per_second') - 1) * 100:+.1f}%. "
text += "This is a small local replay comparison, not statistical proof of a production improvement. Both sides made zero measured Telegram calls and zero measured SQL queries per update. Queue peak was one; every submitted update was counted. Allocation probes remained approximately 7.030 allocations and 89,198 allocated bytes per update on both sides. The earlier first-pass reduction from 15.030 allocations/update is separate.\n\n"
text += f"The final fenced, disk-staged statistics flush persisted all 10,000 groups in {stats_after['seconds']:.3f} seconds with {stats_after['db_statements']} top-level SQL statements ({stats_after['db_statements']/10000:.4f} per counted message), versus {stats_before['seconds']:.3f} seconds and {stats_before['db_statements']} statements before recovery/fencing. No groups remained dirty. This adds durability overhead.\n\n"
text += "The same 10-million-event storage probe with 1,000 subjects per map retained 131,072,000 bytes of timestamp buffers at the old limits and 1,536,000 bytes at the new limits: **98.83% less**. Probe RSS deltas were 128,428 versus 1,928 KiB. This excludes hash-map overhead and all other bot state.\n\n"
text += f"A 60-million-update replay completed in {soak['seconds']:.3f} seconds at {soak['updates_per_second']:,.0f}/second. CPU time was {soak['cpu_seconds']:.2f} seconds ({soak['cpu_percent']:.1f}% of one logical CPU). Sampled p50/p95/p99 were {soak['p50_us']:.3f}/{soak['p95_us']:.3f}/{soak['p99_us']:.3f} microseconds. Peak sampled RSS was {soak['sampled_peak_rss_kib']:,} KiB; final RSS {soak['rss_kib']:,} KiB. All {soak['counted']:,} messages were counted. This is 4.5 minutes of replay, not multi-day operation, and excludes the production model footprint.\n\n"
text += "The initial 10,000-group mirror used about 3.1 MiB above baseline; the 1,000-active-group fixture added about 12.3 MiB, including fixture messages and caches. These deltas are not isolated production per-group measurements. The empty ChatState layout remains 928 bytes before heap allocations.\n\n"
text += "In the simulated downstream-stall workload, unrelated p99 fell from 52.051 ms with the original dispatcher model to 1.207 ms with fair admission. The 20,000-update instantaneous overload still dropped 9,750 raw envelopes in the fair model, with both queues reaching their 4,096 bounds. This is explicit evidence that bounded RAM is not equivalent to lossless overload handling. The isolated eight-thread counter-lock probe measured 187.51 ms aggregate thread wait on one group versus 4.72 ms over 1,000 groups for 160,000 operations; it is not a whole-handler lock profile.\n\n"
text += "**Practical capacity:** the tested cached text path sustained roughly the replay rate above on this WSL host. Live production capacity remains unverified. With the configured application outbound policy of 25 calls/second, a workload averaging `a` outbound-policy calls per update has an additional policy ceiling of approximately `25/a` updates/second before other limits. This is a derived local-policy bound, not a claim about Telegram's MTProto server limits.\n\n"
text += "Validation: 326 enabled bot tests; 14 PostgreSQL integration tests; dedicated ownership/recovery/retention fault probes; 32 grammers library tests (3 client, 4 sender, 25 session); 31 fleet and 17 shard tooling tests. Release build: `cargo build --release --locked`. Two dead-code warnings remain for compatibility/diagnostic helpers.\n"
report = (OUT / "README.md").read_text()
report = report.replace("\n## Remaining release limits, ranked", text + "\n## Remaining release limits, ranked")
(OUT / "README.md").write_text(report)
print(text)
