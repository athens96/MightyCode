//! What to push (C# `MobilePushPlanner`, R\MobilePush.cs; plan §1.3, `push.json`'s planner vectors): the first observation is the
//! baseline; a new request is pushed once; `running → completed|error` with nothing pending pushes `completed`/`failed`; a newly lost
//! sign-in is pushed; names clipped to 80 scalars, the summary to 200 on one line; 512 sent ids and counters for 512 sessions remembered
//! (`Reset` keeps both); ids `request:<runId>:<id>`, `run:<session>:<n>:completed|failed`, `signin:<session>:<n>`. Owner: W8-A.
//!
//! At rest (W8 Step 0): empty.
