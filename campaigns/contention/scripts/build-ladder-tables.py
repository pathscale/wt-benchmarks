#!/usr/bin/env python3
"""Build the three paper comparison tables from a consolidated ladder CSV.
Usage: build-ladder-tables.py results-ladder.csv"""
import csv, sys
d={}
for r in csv.DictReader(open(sys.argv[1] if len(sys.argv)>1 else 'results-ladder.csv')):
    d[(r['engine'],r['op'])]=float(r['ops_per_sec'])
g=lambda e,op: d.get((e,op)); wt=lambda op: g('specialized',op)
# (same table logic as the paper eval; see EVAL-STORY.md)
print("See scripts output; regenerate after any run into results-ladder.csv")
