# Security Audit

Date: 2026-02-23
Repository: `openbot`
Auditor: Codex

## Scope

This audit focused on execution approval and sandbox/permission handling paths in:

- `src/runner.rs`
- `src/config.rs`

## Method

- Reviewed current code paths for approval policy selection and execution approval handling.
- Validated whether runtime approval requests require user consent.
- Cross-checked sandbox config mapping against runtime behavior.

## Findings

### 1. Critical: Execution approval is globally bypassed

**Severity:** Critical  
**Category:** Authorization / Policy Enforcement  
**Affected file:** `src/runner.rs`

#### Evidence

- Approval policy is forced to `AskForApproval::Never` for all sandbox modes:
  - `src/runner.rs:42-45`
- Runtime execution approval requests are auto-approved without user decision:
  - `src/runner.rs:274-283`

Relevant snippets (paraphrased):

- `approval_policy` match returns `Never` for both `DangerFullAccess` and all other modes.
- On `EventMsg::ExecApprovalRequest`, code immediately submits `ReviewDecision::Approved`.

#### Impact

- Eliminates command consent gating entirely.
- Any action that should require explicit approval is executed automatically.
- Increases risk of destructive commands, data exfiltration, and unsafe tool invocation.
- Sandbox mode no longer provides meaningful interactive guardrails when approval should apply.

#### Exploitability

- High. Any workflow that emits an approval request is automatically granted.
- No user interaction is required to proceed.

#### Recommendation

1. Stop forcing `approval_policy` to `Never`.
2. Respect configured/default policy from runtime config.
3. Replace unconditional approval in `ExecApprovalRequest` handling with one of:
   - Explicit user prompt and decision relay, or
   - A safe deny-by-default path unless policy explicitly permits auto-approve.
4. Add tests that verify:
   - Approval-required commands are blocked pending user decision.
   - Denied approvals prevent execution.
   - Sandbox mode and approval policy combinations behave as intended.

#### Suggested Fix Direction

- In `run()`, derive `approval_policy` from config rather than hard-coding `Never`.
- In event loop, remove unconditional:
  - `decision: ReviewDecision::Approved`
- Implement policy-aware handling:
  - `Never`: no approval flow needed.
  - `OnRequest`/equivalent: require explicit user decision.

## Residual Risk

Until the above is fixed, users should assume approval prompts are non-functional and that command execution may proceed without consent.

## Status

- Verified: 1 critical issue
- Additional deep audit (auth/session boundaries, tool allowlisting, prompt injection hardening) not yet completed.
