/** Probe plugin: proves an out-of-repo TS plugin mounts in a dsh profile. */
import type { Context } from '@deepseek-ai/cordis'
import { LlmError } from '@deepseek-ai/dsh-llm'

export const name = 'probe-plugin'
export const inject = ['llm']

export function apply(ctx: Context, config: unknown): void {
  ctx.logger.info(`probe-plugin mounted with config ${JSON.stringify(config)}; LlmError=${typeof LlmError}`)
}
