# RFC template

<!--
Copy this file to docs/src/rfcs/NNNN-short-name.md, replace the heading with
"# NNNN: Title", and fill in each section. Delete the guidance text as you go.

Sections follow the Rust RFC template
(https://github.com/rust-lang/rfcs/blob/master/0000-template.md), cut down for a small
project, with a Security section added after Drawbacks.
-->

- Status: Proposed
- Tracking issue: #N

## Summary

One paragraph: what changes, for whom.

## Motivation

The problem users have today, with concrete use cases. Explain why it belongs in msbx and not
in microsandbox or `msb` (see the scope rule in CONTRIBUTING.md).

## Guide-level explanation

Describe the change as if it had shipped and you were teaching it to a user: the commands, keys
or settings they would use, example output and error messages, and how it changes their
workflow.

## Reference-level explanation

The technical design, in enough detail to implement and review: which modules change, new
types or functions, interaction with existing features and the microsandbox SDK (cite the SDK
source you checked), corner cases, and the tests that will cover it.

## Drawbacks

The case against doing this, including maintenance cost and added surface.

## Security

What the change does to the [security model](../explanation/security-model.md): new paths from
the VM to the host, files or credentials exposed, network access, and how a malicious agent in
the VM could misuse it. State the mitigations and what stays unprotected.

## Rationale and alternatives

Why this design over the alternatives you considered, including doing it upstream in
microsandbox, and the cost of not doing it at all.

## Prior art

How other tools (for example Docker Sandboxes, dev containers, other agent sandboxes) handle
this, and what to learn from them.

## Unresolved questions

What should be settled during review, what during implementation, and what is out of scope.

## Future possibilities

Natural extensions, noted for later. Writing them here does not commit the project to them.
