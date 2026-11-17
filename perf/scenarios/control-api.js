// Control-API scenario (D-018): 80% reads, 20% desired-state writes that create
// operations, offered at a constant arrival rate (an open model: k6 keeps offering load
// whether or not the server keeps up, and counts what it could not send as dropped).
// While it runs, the worker reconciles every operation against the fault-injecting fake
// provider; scripts/load.sh then checks the provider side: zero duplicates.
//
// Env: BASE_URL, TOKEN, MODE (smoke | full), RATE (requests/s), PROJECTS, TOTAL (probes only).
import http from 'k6/http';
import { check, fail } from 'k6';
import { Counter, Rate } from 'k6/metrics';

const MODE = __ENV.MODE || 'smoke';
const RATE = parseInt(__ENV.RATE || '2000', 10);
// TOTAL overrides the count only for plumbing probes (scripts/load.sh keeps those out of perf/results).
const TOTAL = __ENV.TOTAL ? parseInt(__ENV.TOTAL, 10) : MODE === 'full' ? 1000000 : 100000;
const PROJECTS = parseInt(__ENV.PROJECTS || '2000', 10);
const BASE = __ENV.BASE_URL;
const AUTH = { headers: { Authorization: `Bearer ${__ENV.TOKEN}`, 'Content-Type': 'application/json' } };
const DESIRED = JSON.stringify({ databases: ['main'], buckets: ['files'], identity: { clients: ['web'] } });

const refused = new Rate('admission_refused'); // 429 too_many_operations: admission control, counted apart
const unexpected = new Counter('unexpected_status');

export const options = {
  discardResponseBodies: false,
  summaryTrendStats: ['min', 'med', 'avg', 'p(90)', 'p(95)', 'p(99)', 'max'],
  setupTimeout: '180s',
  scenarios: {
    control_api: {
      executor: 'constant-arrival-rate',
      rate: RATE,
      timeUnit: '1s',
      duration: `${Math.ceil(TOTAL / RATE)}s`,
      preAllocatedVUs: 200,
      maxVUs: 2000,
    },
  },
  // Proposed lab targets (END_GOAL evidence plan), not measured capacity. A miss fails
  // the run and is kept as evidence. A plumbing probe (TOTAL set, the gate's short run)
  // checks correctness only: latency on a shared CI runner or a busy laptop is not a gate.
  thresholds: __ENV.TOTAL
    ? {
        'http_req_failed{scenario:control_api}': ['rate<0.001'],
        unexpected_status: ['count<1'],
      }
    : {
        'http_req_failed{scenario:control_api}': ['rate<0.001'],
        'http_req_duration{kind:read}': ['p(95)<25', 'p(99)<100'],
        'http_req_duration{kind:write}': ['p(95)<50', 'p(99)<150'],
        admission_refused: ['rate<0.01'],
        unexpected_status: ['count<1'],
        dropped_iterations: [`count<${Math.ceil(TOTAL * 0.01)}`],
      },
};

http.setResponseCallback(http.expectedStatuses(200, 202, 429));

function project(i) {
  return `load${String(i).padStart(5, '0')}`;
}

export function setup() {
  // One operation per project first, so reads have real operations to read.
  const ids = [];
  for (let start = 0; start < PROJECTS; start += 200) {
    const batch = [];
    for (let i = start; i < Math.min(start + 200, PROJECTS); i++) {
      batch.push(['PUT', `${BASE}/v1/projects/${project(i)}/desired`, DESIRED, AUTH]);
    }
    for (const r of http.batch(batch)) {
      if (r.status !== 202) fail(`setup PUT returned ${r.status}: ${r.body}`);
      ids.push(r.json('operation_id'));
    }
  }
  return { ids };
}

export default function (data) {
  const roll = Math.random();
  if (roll < 0.2) {
    const p = project(Math.floor(Math.random() * PROJECTS));
    const r = http.put(`${BASE}/v1/projects/${p}/desired`, DESIRED, Object.assign({ tags: { kind: 'write', name: 'PUT desired' } }, AUTH));
    refused.add(r.status === 429);
    if (r.status !== 202 && r.status !== 429) unexpected.add(1);
    check(r, { 'write admitted or refused by admission': (x) => x.status === 202 || x.status === 429 });
  } else if (roll < 0.9) {
    const id = data.ids[Math.floor(Math.random() * data.ids.length)];
    const r = http.get(`${BASE}/v1/operations/${id}`, Object.assign({ tags: { kind: 'read', name: 'GET operation' } }, AUTH));
    if (r.status !== 200) unexpected.add(1);
  } else {
    const p = project(Math.floor(Math.random() * PROJECTS));
    const r = http.get(`${BASE}/v1/projects/${p}`, Object.assign({ tags: { kind: 'read', name: 'GET project' } }, AUTH));
    if (r.status !== 200) unexpected.add(1);
  }
}
