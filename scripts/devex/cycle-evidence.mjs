import { appendFile, mkdir } from 'node:fs/promises'
import path from 'node:path'

export async function cycleEvidence(artifacts, workflow, records, events = []) {
  await mkdir(artifacts, { recursive: true })
  const rows = records.map((row) => ({
    scenario: workflow.name,
    worker: row.variables.worker,
    cycle: row.variables.cycle,
    business_duration_ms: row.elapsed,
    request_duration_ms: row.requestElapsed ?? row.elapsed,
    succeeded: row.succeeded,
    proof: row.proof,
    ...(row.failure ? { failure: row.failure } : {}),
    ...(!row.proof ? { proof_failure: { category: 'job_timing_evidence_invalid' } } : {}),
  }))
  if (rows.length)
    await appendFile(
      path.join(artifacts, 'cycles.jsonl'),
      rows.map(JSON.stringify).join('\n') + '\n',
    )
  if (events.length)
    await appendFile(
      path.join(artifacts, 'session-events.jsonl'),
      events.map(JSON.stringify).join('\n') + '\n',
    )
}
