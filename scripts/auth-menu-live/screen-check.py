#!/usr/bin/env python3
"""Correlate a live.mjs run with windows.swift samples: which windows came
and went on screen, and whether the frontmost app changed, while the login
ran. Usage: screen-check.py <evidence.json> <samples.jsonl>"""
import json, sys

evidence = json.load(open(sys.argv[1]))
run = evidence["runs"][-1]
start, end = run["loginStartedAt"], run["loginEndedAt"]
samples = [json.loads(l) for l in open(sys.argv[2]) if l.startswith("{")]
before = [s for s in samples if s["at"] <= start]
base = before[-1]["state"] if before else samples[0]["state"]
key = lambda w: w.split("@")[0]
seen = {key(w) for w in base["windows"]}
fronts, appeared = {base["front"]}, {}
for s in samples:
    if not start <= s["at"] <= end:
        continue
    fronts.add(s["state"]["front"])
    for w in s["state"]["windows"]:
        if key(w) not in seen:
            ws = s["state"]["windows"]
            # How many windows of other apps are in front of it (front to back).
            ahead = sum(1 for x in ws[: ws.index(w)] if "Chrome for Testing" not in x and not x.startswith("Dock"))
            appeared.setdefault(key(w), [s["at"] - start, s["at"] - start, w, ahead])
            appeared[key(w)][1] = s["at"] - start
            appeared[key(w)][3] = min(appeared[key(w)][3], ahead)
closes = [e["at"] - start for e in run["events"] if e.get("kind") == "close" and e.get("why") == "elsewhere"]
print(json.dumps({
    "loginMs": end - start,
    "menuClosedElsewhereAtMs": closes,
    "frontmostBefore": base["front"],
    "frontmostDuring": sorted(fronts),
    "windowsThatAppeared": {k: {"firstMs": v[0], "lastMs": v[1], "window": v[2], "otherAppWindowsInFront": v[3]} for k, v in appeared.items()},
}, indent=1))
