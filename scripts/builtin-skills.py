#!/usr/bin/env python3
"""Writes crates/ca-agent/src/builtin_skills.rs from crates/ca-agent/skills/*/SKILL.md (the built-in skills).
Run it after adding or removing a built-in skill: python3 scripts/builtin-skills.py"""
import os
base = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "crates", "ca-agent")
names = sorted(d for d in os.listdir(os.path.join(base, "skills")) if os.path.isfile(os.path.join(base, "skills", d, "SKILL.md")))
lines = ["//! The skills that come with Code Arena: Arena's own (its repository's `skills/` folder), built in. Written by",
         "//! `scripts/builtin-skills.py`; a test checks it lists every folder in `skills/`.", "",
         "/// Each built-in skill: its name, and its SKILL.md.", "pub const ALL: &[(&str, &str)] = &["]
lines += [f'    ("{n}", include_str!("../skills/{n}/SKILL.md")),' for n in names]
lines.append("];")
open(os.path.join(base, "src", "builtin_skills.rs"), "w").write("\n".join(lines) + "\n")
