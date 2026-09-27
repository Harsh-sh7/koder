// Minimal OpenAI-compatible chat server used to exercise the harness agent loop
// without network access or credentials. It answers
// `POST /v1/chat/completions` with a canned reply, streaming when asked, and
// can answer with a tool call so a run exercises the tool path too.
//
// Usage: node mock-openai.mjs [--port 8899] [--tool | --spec | --wave | --panel | --fail]
//   --tool   first reply calls the `bash` tool (`echo harness-ok`), later replies are text.
//   --spec   first reply calls `spec_get`, second calls approval-gated `spec_amend`, then text.
//   --wave   first reply calls `wave_plan` over a three-part job with one shared file,
//            then text; every request reports the tool names it was offered and whether
//            the operating-contract section reached it, which is the harness proof.
//   --panel  walks the agent-flow surfaces: a task list (twice, so the live card
//            reflects the newest), a `subagent` delegation, a created file, an
//            `edit` of it, and a command held open — the panel's two lists and
//            the transcript's delegation, change, and to-do cards with live rows.
//   --build  exercises run_wave end to end: the parent hands three parts to the
//            harness, each part's child writes its own file and reports through
//            structured_output, and the parent closes with a rich markdown answer
//            (headings, lists, a table, code, a mermaid block, an ASCII diagram).
//   --fail   answers every request with the 429 body OpenRouter returns when a
//            free route's shared pool is exhausted, so the failure path — the
//            message the app shows and the retry it offers — is provable too.

import { createServer } from 'node:http'
import { appendFileSync } from 'node:fs'
import { createHash } from 'node:crypto'

const argv = process.argv.slice(2)
const portIndex = argv.indexOf('--port')
const recordIndex = argv.indexOf('--record')
const recordPath = recordIndex === -1 ? undefined : argv[recordIndex + 1]
const port = portIndex === -1 ? 8899 : Number(argv[portIndex + 1])
const useTool = argv.includes('--tool')
const useSpec = argv.includes('--spec')
const useWave = argv.includes('--wave')
const usePanel = argv.includes('--panel')
const failMode = argv.includes('--fail')
const useBuild = argv.includes('--build')

/** The parts the build mode hands to run_wave, by id: the file each owns. */
const BUILD_PARTS = {
  core: '.harness/ui-check/todo.py',
  tests: '.harness/ui-check/test_todo.py',
  docs: '.harness/ui-check/README.md',
}

/** The answer the build mode closes with: every markdown shape the transcript draws. */
const BUILD_ANSWER = [
  '## Todo CLI is built',
  '',
  'Three parts ran as **parallel subagents**, each confined to its own file; the `run_wave` result above has one line per part.',
  '',
  '### What changed',
  '',
  '- `todo.py` — `add`, `list`, and `done` commands over a JSON store',
  '- `test_todo.py` — 6 tests, including the empty and malformed cases',
  '- `README.md` — how to run it',
  '',
  '1. Parse the command',
  '2. Load the store',
  '    - create it when missing',
  '3. Apply and save',
  '',
  '| Part | File | Check | Result |',
  '| --- | --- | --- | --- |',
  '| core | todo.py | `python todo.py list` | pass |',
  '| tests | test_todo.py | `pytest -q` | 6 passed |',
  '| docs | README.md | — | written |',
  '',
  '> Verified by running both checks after the wave settled.',
  '',
  '```python',
  'def add(store: list[dict], title: str) -> dict:',
  '    """Add a todo; blank titles are refused."""',
  '    if not title.strip():',
  '        raise ValueError("a todo needs a title")',
  '    item = {"id": len(store) + 1, "title": title.strip(), "done": False}',
  '    store.append(item)',
  '    return item',
  '```',
  '',
  '```mermaid',
  'flowchart LR',
  '  CLI[todo.py] -->|reads| Store[(todos.json)]',
  '  Tests[test_todo.py] --> CLI',
  '```',
  '',
  '```text',
  '+---------+     +-------------+     +------------+',
  '|  add    | --> |  todos.json | <-- |  list/done |',
  '+---------+     +-------------+     +------------+',
  '```',
].join('\n')

/**
 * The reply for one request in build mode.
 *
 * Parent and children are told apart by what they were offered: only a
 * structured child has `structured_output`. Every decision is read off the
 * request itself, because the children run at the same time and a shared
 * counter would interleave them.
 *
 * @param body the request
 * @returns a reply in the shape `replyFor` expects
 */
function buildReply(body) {
  const messages = body.messages ?? []
  const text = JSON.stringify(messages)
  const offered = (body.tools ?? []).map(tool => tool.function?.name)
  const toolResults = messages.filter(message => message.role === 'tool').length
  const call = (id, name, args) => ({ toolCall: { id, type: 'function', function: { name, arguments: JSON.stringify(args) } } })
  if (offered.includes('structured_output')) {
    const part = /Your part, \\?"([a-z]+)\\?"/u.exec(text)?.[1] ?? 'core'
    process.stdout.write(`build-mode: child ${part} step ${String(toolResults + 1)}\n`)
    if (toolResults === 0) {
      return call(`call_write_${part}`, 'write', { file_path: BUILD_PARTS[part], content: `# ${part}\n\nbuilt by the ${part} part\n` })
    }
    return call(`call_done_${part}`, 'structured_output', { status: 'done', summary: `${BUILD_PARTS[part]} written; its check passed.` })
  }
  if (toolResults === 0) {
    process.stdout.write('build-mode: parent run_wave\n')
    return call('call_run_wave_1', 'run_wave', {
      parts: [
        { id: 'core', task: 'Write the todo CLI with add, list and done.', files: [BUILD_PARTS.core], check: 'python todo.py list' },
        { id: 'tests', task: 'Write pytest tests for the CLI, including empty and malformed input.', files: [BUILD_PARTS.tests], check: 'pytest -q' },
        { id: 'docs', task: 'Write a README that says how to run the CLI and its tests.', files: [BUILD_PARTS.docs] },
      ],
    })
  }
  process.stdout.write('build-mode: parent answer\n')
  return { text: BUILD_ANSWER }
}

/** The replacement spec the mock proposes; the probe greps the log for it. */
const AMENDED_SPEC = 'Amended frozen spec: build the terminal harness with a live token meter.'

/**
 * Which spec-lock action the request still lacks, read straight off the
 * conversation: the probe reads the spec, amends it with approval, and reads
 * again — so the second read must serve the amended text.
 */
function specAction() {
  // Monotonic on purpose: compaction rewrites the visible history, so deciding
  // from what the request still contains would flip back and forth forever.
  const plan = [
    { id: 'call_spec_get_1', name: 'spec_get', arguments: '{}' },
    {
      id: 'call_spec_amend_1',
      name: 'spec_amend',
      arguments: JSON.stringify({ text: AMENDED_SPEC, reason: 'prove the approval gate end to end' }),
    },
    { id: 'call_spec_get_2', name: 'spec_get', arguments: '{}' },
  ]
  const next = plan[emitted]
  emitted += 1
  process.stdout.write(`spec-mode: ${next?.id ?? 'text'} (#${String(emitted)})\n`)
  return next
}

const reasonings = 'Mock reasoning: the request reached a model, so the protocol, credential and stream path are all live.'

/** First line of the harness operating contract, the section the wave-mode probe proves is live. */
const CONTRACT_MARKER = 'Operating contract of this harness:'

/**
 * Which wave-plan action the request still lacks. One call is enough to prove the
 * partitioner executes and renders; the reply after it is plain text.
 */
function waveAction() {
  const plan = [
    {
      id: 'call_wave_plan_1',
      name: 'wave_plan',
      arguments: JSON.stringify({
        subtasks: [
          { id: 'store', files: ['src/store.ts'] },
          { id: 'ui', files: ['src/App.tsx'] },
          // Shares src/store.ts with `store`, so the plan must order these two
          // apart and report the conflict rather than wave them together.
          { id: 'store-tests', files: ['src/store.ts', 'src/store.test.ts'] },
        ],
      }),
    },
  ]
  const next = plan[emitted]
  emitted += 1
  process.stdout.write(`wave-mode: ${next?.id ?? 'text'} (#${String(emitted)})\n`)
  return next
}

/** The task handed to the subagent, and the marker that identifies the child's own request. */
const PANEL_TASK = 'audit the token meter'

/**
 * The child's own prompt. The parent's replay also carries this text — it is the
 * subagent call's argument — so what tells the child apart is that the child has
 * no tool result yet: its conversation is the prompt and nothing more.
 */
const CHILD_PROMPT = 'Read src/store.ts and list every place tokens are spent.'

/**
 * The reply for one request in panel mode: the next planned call, the child's
 * own answer, or the closing line.
 *
 * The plan walks the interface through every card shape the session panel and
 * the transcript can draw: a task list twice (so the live card reflects the
 * newest), a delegation, a created file, an edit, and finally a command held
 * open so the panel has a running row.
 *
 * @param messages the conversation the request replayed
 * @returns a reply in the shape `replyFor` expects
 */
function panelReply(messages) {
  const text = JSON.stringify(messages)
  const sawToolResult = messages.some(message => message.role === 'tool')
  if (text.includes(CHILD_PROMPT) && !sawToolResult) {
    process.stdout.write(`panel-mode: child turn (#${String(emitted)})\n`)
    return { text: 'The meter counts every call once: nothing spends tokens unrecorded.' }
  }
  const todo = (steps) => JSON.stringify({ todos: steps })
  const plan = [
    {
      id: 'call_todo_1',
      name: 'todo_write',
      arguments: todo([
        { content: 'audit the token meter', status: 'in_progress' },
        { content: 'fix the double count in store.ts', status: 'pending' },
        { content: 'run the tests', status: 'pending' },
      ]),
    },
    {
      id: 'call_subagent_1',
      name: 'subagent',
      arguments: JSON.stringify({
        description: 'audit the token meter',
        prompt: CHILD_PROMPT,
      }),
    },
    {
      id: 'call_write_1',
      name: 'write',
      arguments: JSON.stringify({
        file_path: '.harness/ui-check/meter.md',
        content: '# meter notes\n\nOne call, one count.\nNothing spends tokens unrecorded.\n',
      }),
    },
    {
      id: 'call_edit_1',
      name: 'edit',
      arguments: JSON.stringify({
        file_path: '.harness/ui-check/meter.md',
        old_string: 'One call, one count.',
        new_string: 'One call, one count — fixed after the audit.',
      }),
    },
    {
      id: 'call_todo_2',
      name: 'todo_write',
      arguments: todo([
        { content: 'audit the token meter', status: 'completed' },
        { content: 'fix the double count in store.ts', status: 'in_progress' },
        { content: 'run the tests', status: 'pending' },
      ]),
    },
    {
      id: 'call_bash_1',
      name: 'bash',
      arguments: JSON.stringify({
        command: 'sleep 120',
        description: 'hold a process open so the panel shows a running row',
      }),
    },
  ]
  const next = plan[emitted]
  emitted += 1
  process.stdout.write(`panel-mode: ${next?.id ?? 'text'} (#${String(emitted)})\n`)
  if (next === undefined) {
    return { text: 'Both panel lists have a live row now.' }
  }
  return {
    toolCall: {
      id: next.id,
      type: 'function',
      function: { name: next.name, arguments: next.arguments },
    },
  }
}

function chunk(delta, finishReason = null, usage = undefined) {
  return {
    id: 'chatcmpl-mock',
    object: 'chat.completion.chunk',
    created: Math.floor(Date.now() / 1000),
    model: 'mock-model',
    choices: finishReason === null && delta === undefined ? [] : [{ index: 0, delta: delta ?? {}, finish_reason: finishReason }],
    ...usage === undefined ? {} : { usage },
  }
}

/** How many spec-lock actions this mock process has already emitted. */
let emitted = 0

/** Plain text of one replayed message, whatever shape its content arrived in. */
function textOf(message) {
  if (typeof message.content === 'string') return message.content
  return (message.content ?? []).map(part => part?.text ?? '').join('')
}

/** Whether this request is the compaction engine asking for a checkpoint. */
function isCompactionRequest(messages) {
  return messages.some(message => message.role === 'user' && textOf(message).includes('acting as a compaction engine'))
}

/** Reply for one request: a tool call on the first turn when asked for, else text. */
function replyFor(body) {
  const toolResults = (body.messages ?? []).filter(message => message.role === 'tool')
  if (isCompactionRequest(body.messages ?? [])) {
    return { text: '<compacted-summary>\n## Primary Request and Intent\n- [mock checkpoint]\n</compacted-summary>' }
  }
  const sawToolResult = toolResults.length > 0
  if (usePanel) {
    return panelReply(body.messages ?? [])
  }
  if (useWave) {
    const offered = (body.tools ?? []).map(tool => tool.function?.name ?? tool.name).filter(name => name !== undefined)
    process.stdout.write(`wave-mode: tools offered: ${offered.join(', ')}\n`)
    process.stdout.write(`wave-mode: contract section present: ${String(JSON.stringify(body.messages ?? []).includes(CONTRACT_MARKER))}\n`)
    const action = waveAction()
    if (action !== undefined) {
      return {
        toolCall: {
          id: action.id,
          type: 'function',
          function: { name: action.name, arguments: action.arguments },
        },
      }
    }
    return { text: 'Wave plan returned; the harness composition is live.' }
  }
  if (useSpec) {
    const action = specAction()
    if (action !== undefined) {
      return {
        toolCall: {
          id: action.id,
          type: 'function',
          function: { name: action.name, arguments: action.arguments },
        },
      }
    }
    return { text: 'Spec-lock flow complete.' }
  }
  if (useBuild) return buildReply(body)
  if (useTool && !sawToolResult) {
    return {
      toolCall: {
        id: 'call_mock_1',
        type: 'function',
        function: {
          name: 'bash',
          arguments: JSON.stringify({ command: 'echo harness-ok', description: 'Prove the shell tool runs end to end' }),
        },
      },
    }
  }
  return { text: `Mock model online. I received ${(body.messages ?? []).length} message(s).` }
}

const server = createServer((request, response) => {
  if (request.method !== 'POST' || !request.url.startsWith('/v1/chat/completions')) {
    response.writeHead(404, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ error: { message: `no route for ${request.method} ${request.url}` } }))
    return
  }
  const chunks = []
  request.on('data', (piece) => chunks.push(piece))
  request.on('end', () => {
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    if (recordPath) appendFileSync(recordPath, JSON.stringify({
      body,
      credentialHash: createHash('sha256').update(request.headers.authorization ?? '').digest('hex'),
    }) + '\n')
    if (failMode) {
      // The body is OpenRouter's, byte for byte in shape: the app's own message
      // has to survive a real provider's error, not a tidy one.
      response.writeHead(429, { 'content-type': 'application/json' })
      response.end(JSON.stringify({
        message: 'Provider returned error',
        code: 429,
        metadata: {
          raw: 'mock/rate-limited-route is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits.',
          provider_name: 'Mock',
          is_byok: false,
          limit_source: 'upstream_provider_shared_pool',
        },
      }))
      return
    }
    const reply = replyFor(body)
    const usage = {
      prompt_tokens: 101,
      completion_tokens: 42,
      total_tokens: 143,
      prompt_tokens_details: { cached_tokens: 64 },
    }
    if (body.stream !== true) {
      response.writeHead(200, { 'content-type': 'application/json' })
      response.end(JSON.stringify({
        id: 'chatcmpl-mock',
        object: 'chat.completion',
        created: Math.floor(Date.now() / 1000),
        model: 'mock-model',
        choices: [{ index: 0, message: reply.toolCall === undefined ? { role: 'assistant', content: reply.text } : { role: 'assistant', content: null, tool_calls: [reply.toolCall] }, finish_reason: reply.toolCall === undefined ? 'stop' : 'tool_calls' }],
        usage,
      }))
      return
    }
    response.writeHead(200, {
      'content-type': 'text/event-stream',
      'cache-control': 'no-cache',
      connection: 'keep-alive',
    })
    const write = (payload) => response.write(`data: ${JSON.stringify(payload)}\n\n`)
    if (body.reasoning_effort !== undefined || body.model?.includes('reason')) write(chunk({ reasoning_content: reasonings }))
    if (reply.toolCall === undefined) {
      for (const piece of reply.text.split(' ')) write(chunk({ content: `${piece} ` }))
      write(chunk({}, 'stop'))
    } else {
      write(chunk({ tool_calls: [{ index: 0, id: reply.toolCall.id, type: 'function', function: { name: reply.toolCall.function.name, arguments: '' } }] }))
      write(chunk({ tool_calls: [{ index: 0, function: { arguments: reply.toolCall.function.arguments } }] }))
      write(chunk({}, 'tool_calls'))
    }
    if (body.stream_options?.include_usage === true) write(chunk(undefined, null, usage))
    response.write('data: [DONE]\n\n')
    response.end()
  })
})

server.listen(port, '127.0.0.1', () => {
  process.stdout.write(`mock OpenAI listening on http://127.0.0.1:${server.address().port}/v1 (tool mode: ${useTool})\n`)
})
