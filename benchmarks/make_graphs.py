#!/usr/bin/env python3
"""Generate benchmark graphs from CSV results.

Usage:
    python3 benchmarks/make_graphs.py [csv_file]

Produces PNG graphs in benchmarks/graphs/.
"""

import os
import sys
import csv
from collections import defaultdict
from statistics import median, mean

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

CSV_PATH = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.dirname(__file__), "benchmark_results.csv"
)
OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "graphs")
os.makedirs(OUT_DIR, exist_ok=True)


def load_csv(path):
    """Load benchmark CSV into structured data.

    Returns: dict of pattern_name -> {category, cold: {tgrep:[], rg:[]},
    warm: {tgrep:[], rg:[]}, files}
    """
    data = defaultdict(lambda: {
        "category": "",
        "cold": {"tgrep": [], "rg": [], "files": 0},
        "warm": {"tgrep": [], "rg": [], "files": 0},
    })
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            name = row["pattern"]
            cat = row["category"]
            cond = row["condition"]
            data[name]["category"] = cat
            data[name][cond]["tgrep"].append(int(row["tgrep_ms"]))
            data[name][cond]["rg"].append(int(row["rg_ms"]))
            data[name][cond]["files"] = int(row["files_matched"])
    return data


def calc_stats(values):
    """Calculate min, median, p95, max for a list of values."""
    if not values:
        return 0, 0, 0, 0
    s = sorted(values)
    n = len(s)
    return (
        s[0],
        int(median(s)),
        s[int(n * 0.95)] if n > 1 else s[0],
        s[-1],
    )


def graph_cold_vs_warm(data):
    """Bar chart: cold vs warm tgrep median, with rg baseline."""
    names = [n for n in sorted(data) if data[n]["cold"]["tgrep"]]
    cats = [data[n]["category"] for n in names]
    tgrep_cold = [median(data[n]["cold"]["tgrep"]) for n in names]
    tgrep_warm = [median(data[n]["warm"]["tgrep"]) if data[n]["warm"]["tgrep"] else 0 for n in names]
    rg_med = [median(data[n]["cold"]["rg"]) for n in names]

    x = np.arange(len(names))
    w = 0.25

    fig, ax = plt.subplots(figsize=(14, 7))
    ax.bar(x - w, rg_med, w, label="rg", color="#888888", alpha=0.8)
    ax.bar(x, tgrep_cold, w, label="tgrep cold", color="#CC0000", alpha=0.8)
    ax.bar(x + w, tgrep_warm, w, label="tgrep warm", color="#FFAA00", alpha=0.8)

    ax.set_ylabel("Median latency (ms)")
    ax.set_title("tgrep vs ripgrep: Cold vs Warm Cache (median latency)")
    ax.set_xticks(x)
    ax.set_xticklabels(names, rotation=45, ha="right", fontsize=8)
    ax.legend()
    ax.set_yscale("log")
    ax.set_ylim(bottom=1)
    ax.grid(axis="y", alpha=0.3)
    fig.tight_layout()
    fig.savefig(os.path.join(OUT_DIR, "cold_vs_warm.png"), dpi=150)
    plt.close(fig)
    print(f"  graphs/cold_vs_warm.png")


def graph_speedup_by_category(data):
    """Grouped bar chart: speedup by category (cold and warm)."""
    categories = defaultdict(lambda: {"cold": [], "warm": []})
    for name in sorted(data):
        cat = data[name]["category"]
        rg_cold = median(data[name]["cold"]["rg"]) if data[name]["cold"]["rg"] else 1
        rg_warm = median(data[name]["warm"]["rg"]) if data[name]["warm"]["rg"] else rg_cold
        t_cold = median(data[name]["cold"]["tgrep"]) if data[name]["cold"]["tgrep"] else 1
        t_warm = median(data[name]["warm"]["tgrep"]) if data[name]["warm"]["tgrep"] else 1
        categories[cat]["cold"].append(rg_cold / max(t_cold, 1))
        categories[cat]["warm"].append(rg_warm / max(t_warm, 1))

    cat_names = sorted(categories.keys())
    cold_means = [mean(categories[c]["cold"]) for c in cat_names]
    warm_means = [mean(categories[c]["warm"]) for c in cat_names]

    x = np.arange(len(cat_names))
    w = 0.35

    fig, ax = plt.subplots(figsize=(10, 6))
    bars1 = ax.bar(x - w/2, cold_means, w, label="Cold cache", color="#CC0000", alpha=0.8)
    bars2 = ax.bar(x + w/2, warm_means, w, label="Warm cache", color="#FFAA00", alpha=0.8)

    ax.axhline(y=1, color="#464646", linestyle="--", alpha=0.5, label="Parity (1x)")
    ax.axhline(y=10, color="#282828", linestyle=":", alpha=0.5, label="10x target")

    ax.set_ylabel("Speedup vs rg (x, log scale)")
    ax.set_title("tgrep Speedup by Pattern Category")
    ax.set_xticks(x)
    ax.set_xticklabels(cat_names, fontsize=9)
    ax.legend()
    ax.set_yscale("log")
    ax.grid(axis="y", alpha=0.3)

    for bars in (bars1, bars2):
        for bar in bars:
            h = bar.get_height()
            ax.text(bar.get_x() + bar.get_width()/2, h, f"{h:.1f}x",
                    ha="center", va="bottom", fontsize=7)

    fig.tight_layout()
    fig.savefig(os.path.join(OUT_DIR, "speedup_by_category.png"), dpi=150)
    plt.close(fig)
    print(f"  graphs/speedup_by_category.png")


def graph_per_pattern_speedup(data):
    """Horizontal bar chart: speedup per pattern (cold cache)."""
    names = sorted(data)
    speedups = []
    for n in names:
        rg = median(data[n]["cold"]["rg"]) if data[n]["cold"]["rg"] else 1
        tg = median(data[n]["cold"]["tgrep"]) if data[n]["cold"]["tgrep"] else 1
        speedups.append(rg / max(tg, 1))

    colors = ["#CC0000" if s >= 1 else "#660000" for s in speedups]

    fig, ax = plt.subplots(figsize=(10, 8))
    y = np.arange(len(names))
    ax.barh(y, speedups, color=colors, alpha=0.8)
    ax.axvline(x=1, color="#464646", linestyle="--", alpha=0.5)
    ax.set_xlabel("Speedup vs rg (x, log scale)")
    ax.set_title("tgrep Cold-Cache Speedup per Pattern")
    ax.set_yticks(y)
    ax.set_yticklabels(names, fontsize=8)
    ax.set_xscale("log")
    ax.grid(axis="x", alpha=0.3)

    for i, s in enumerate(speedups):
        ax.text(s, i, f" {s:.1f}x", va="center", fontsize=7)

    fig.tight_layout()
    fig.savefig(os.path.join(OUT_DIR, "speedup_per_pattern.png"), dpi=150)
    plt.close(fig)
    print(f"  graphs/speedup_per_pattern.png")


def graph_latency_scatter(data):
    """Scatter plot: tgrep latency vs rg latency, colored by category."""
    cat_colors = {
        "rare": "#CC0000",
        "broad": "#FFAA00",
        "word": "#282828",
        "caseinsensitive": "#888888",
    }

    fig, ax = plt.subplots(figsize=(8, 8))
    for name in sorted(data):
        cat = data[name]["category"]
        color = cat_colors.get(cat, "#464646")
        rg = median(data[name]["cold"]["rg"]) if data[name]["cold"]["rg"] else 0
        tg = median(data[name]["cold"]["tgrep"]) if data[name]["cold"]["tgrep"] else 0
        ax.scatter(rg, tg, c=color, s=60, alpha=0.8, label=cat, edgecolors="#1A1A1A", linewidths=0.5)

    # Parity line
    max_val = max(
        max(median(d["cold"]["rg"]) for d in data.values() if d["cold"]["rg"]),
        max(median(d["cold"]["tgrep"]) for d in data.values() if d["cold"]["tgrep"]),
    )
    ax.plot([0, max_val * 1.1], [0, max_val * 1.1], "--", color="#464646", alpha=0.5, label="Parity")

    ax.set_xlabel("rg latency (ms)")
    ax.set_ylabel("tgrep latency (ms)")
    ax.set_title("tgrep vs rg Latency (cold cache, median)")
    ax.set_xscale("log")
    ax.set_yscale("log")
    ax.grid(alpha=0.3)

    # Deduplicate legend
    handles, labels = ax.get_legend_handles_labels()
    seen = set()
    unique = [(h, l) for h, l in zip(handles, labels) if l not in seen and not seen.add(l)]
    ax.legend(*zip(*unique))

    fig.tight_layout()
    fig.savefig(os.path.join(OUT_DIR, "latency_scatter.png"), dpi=150)
    plt.close(fig)
    print(f"  graphs/latency_scatter.png")


def graph_trial_distribution(data):
    """Box plot: per-trial latency distribution for top patterns."""
    names = [n for n in sorted(data) if data[n]["warm"]["tgrep"]]
    # Pick a representative subset (one per category + all word patterns)
    subset = [n for n in names if data[n]["category"] in ("word",)]
    subset += [n for n in names if data[n]["category"] == "rare"][:3]
    subset += [n for n in names if data[n]["category"] == "broad"][:2]
    subset += [n for n in names if data[n]["category"] == "caseinsensitive"][:2]

    fig, ax = plt.subplots(figsize=(12, 6))
    positions = []
    boxes = []
    pos = 0
    for n in subset:
        warm = data[n]["warm"]["tgrep"]
        if warm:
            boxes.append(warm)
            positions.append(pos)
            pos += 1.5
        cold = data[n]["cold"]["tgrep"]
        if cold:
            boxes.append(cold)
            positions.append(pos)
            pos += 1.5

    bp = ax.boxplot(boxes, positions=positions, widths=1.0, patch_artist=True)
    for i, patch in enumerate(bp["boxes"]):
        # Alternate cold (red) and warm (amber)
        patch.set_facecolor("#CC0000" if i % 2 == 0 else "#FFAA00")
        patch.set_alpha(0.7)

    ax.set_ylabel("Latency (ms)")
    ax.set_title("tgrep Latency Distribution (cold=red, warm=amber)")
    ax.set_yscale("log")
    ax.set_ylim(bottom=1)
    ax.grid(axis="y", alpha=0.3)
    fig.tight_layout()
    fig.savefig(os.path.join(OUT_DIR, "trial_distribution.png"), dpi=150)
    plt.close(fig)
    print(f"  graphs/trial_distribution.png")


def generate_summary_table(data):
    """Generate a markdown summary table from the benchmark data."""
    lines = []
    lines.append("| Pattern | Category | tgrep cold (ms) | tgrep warm (ms) | rg (ms) | Cold speedup | Warm speedup | Files |")
    lines.append("|---------|----------|-----------------|-----------------|---------|-------------|-------------|-------|")
    for name in sorted(data):
        d = data[name]
        cat = d["category"]
        t_cold = median(d["cold"]["tgrep"]) if d["cold"]["tgrep"] else 0
        t_warm = median(d["warm"]["tgrep"]) if d["warm"]["tgrep"] else 0
        rg = median(d["cold"]["rg"]) if d["cold"]["rg"] else 0
        cold_sp = f"{rg/max(t_cold,1):.1f}x" if t_cold else "-"
        warm_sp = f"{rg/max(t_warm,1):.1f}x" if t_warm else "-"
        files = d["cold"]["files"]
        lines.append(f"| {name} | {cat} | {t_cold:.0f} | {t_warm:.0f} | {rg:.0f} | {cold_sp} | {warm_sp} | {files} |")
    return "\n".join(lines)


def main():
    print(f"Loading {CSV_PATH}...")
    data = load_csv(CSV_PATH)
    print(f"Loaded {len(data)} patterns")
    print(f"Generating graphs in {OUT_DIR}/...")

    graph_cold_vs_warm(data)
    graph_speedup_by_category(data)
    graph_per_pattern_speedup(data)
    graph_latency_scatter(data)
    graph_trial_distribution(data)

    # Write summary table
    table = generate_summary_table(data)
    table_path = os.path.join(OUT_DIR, "..", "summary_table.md")
    with open(table_path, "w") as f:
        f.write(table + "\n")
    print(f"  summary_table.md")

    print("Done.")


if __name__ == "__main__":
    main()
