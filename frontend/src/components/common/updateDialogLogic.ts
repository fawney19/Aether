import type { SystemUpdatePreflightResponse } from '@/api/admin'

const DEFAULT_DOCKER_UPDATE_COMMAND = 'bash ./update.sh'

export function isPreflightBlocking(
  data: SystemUpdatePreflightResponse | null | undefined
): boolean {
  if (!data) return false
  if (data.can_apply_update === false) return true
  return data.checks.some((item) => item.status === 'blocked')
}

export interface DockerGuidedCommandsInput {
  updateCommand?: string | null
  prepareCommand?: string | null
  applyCommand?: string | null
}

export interface DockerGuidedCommands {
  updateCommand: string
  prepareCommand: string
  applyCommand: string
}

function normalizeCommand(value?: string | null): string | null {
  const trimmed = value?.trim()
  return trimmed ? trimmed : null
}

export function appendDockerUpdateFlag(command: string, flag: string): string {
  const normalizedCommand = normalizeCommand(command) || DEFAULT_DOCKER_UPDATE_COMMAND
  if (normalizedCommand.split(/\s+/).includes(flag)) return normalizedCommand
  return `${normalizedCommand} ${flag}`
}

export function buildDockerGuidedCommands(
  input: DockerGuidedCommandsInput
): DockerGuidedCommands | null {
  const updateCommand = normalizeCommand(input.updateCommand)
  const prepareCommand = normalizeCommand(input.prepareCommand)
  const applyCommand = normalizeCommand(input.applyCommand)

  if (!updateCommand && !prepareCommand && !applyCommand) {
    return null
  }

  const baseCommand = updateCommand || DEFAULT_DOCKER_UPDATE_COMMAND
  return {
    updateCommand: updateCommand || baseCommand,
    prepareCommand: prepareCommand || appendDockerUpdateFlag(baseCommand, '--prepare'),
    applyCommand: applyCommand || appendDockerUpdateFlag(baseCommand, '--apply-prepared'),
  }
}
