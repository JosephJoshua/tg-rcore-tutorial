Complete all 5 basic OS kernel exercises in this repository. Work through them in order: ch3, ch4, ch5, ch6, ch8.

For each chapter:

1. Read `tg-rcore-tutorial-chN/exercise.md` for the full specification
2. Read the existing source code in `tg-rcore-tutorial-chN/src/` before making changes
3. Implement the solution — only modify files under `tg-rcore-tutorial-chN/src/` unless exercise.md says otherwise (ch4 and ch6 require cloning dependency crates locally)
4. Test with `cd tg-rcore-tutorial-chN && bash test.sh exercise`
5. Also run `bash test.sh base` to verify base tests still pass (forward compatibility is required from ch5 onward)

User test programs in `tg-rcore-tutorial-user/` are auto-fetched at build time — do NOT modify them.

After completing each chapter, briefly document what you implemented, any bugs you encountered, and how you resolved them. Write that document to docs/reports/exercise-chN.md. The final deliverable for each chapter should be a publishable crate — verify with `cargo publish --dry-run`.
