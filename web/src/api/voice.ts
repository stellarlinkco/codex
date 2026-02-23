/**
 * API functions for voice assistant integration.
 *
 * Fetches conversation tokens from the hub for ElevenLabs integration.
 * The hub handles authentication with ElevenLabs API, keeping credentials secure.
 *
 * Supports two modes:
 * 1. Default: Hub uses its own ElevenLabs credentials (production)
 * 2. Custom: Client provides their own ElevenLabs agent ID and API key
 */

import type { ApiClient } from './client'
import {
    ELEVENLABS_API_BASE,
    VOICE_AGENT_NAME,
    buildVoiceAgentConfig
} from '@codex/protocol/voice'

export interface VoiceTokenResponse {
    allowed: boolean
    token?: string
    agentId?: string
    error?: string
}

export interface VoiceTokenRequest {
    customAgentId?: string
    customApiKey?: string
}

/**
 * Fetch a conversation token from the hub for ElevenLabs voice sessions.
 *
 * This uses the private agent flow where:
 * 1. Hub holds the ELEVENLABS_API_KEY and ELEVENLABS_AGENT_ID (or uses user-provided ones)
 * 2. Hub fetches a short-lived conversation token from ElevenLabs
 * 3. Client uses this token to establish WebRTC connection
 */
export async function fetchVoiceToken(
    api: ApiClient,
    options?: VoiceTokenRequest
): Promise<VoiceTokenResponse> {
    try {
        return await api.fetchVoiceToken(options)
    } catch (error) {
        return {
            allowed: false,
            error: error instanceof Error ? error.message : 'Network error'
        }
    }
}

export interface ElevenLabsAgent {
    agent_id: string
    name: string
}

export interface FindAgentResult {
    success: boolean
    agentId?: string
    error?: string
}

export interface CreateAgentResult {
    success: boolean
    agentId?: string
    error?: string
    created?: boolean
}

/**
 * Find an existing voice agent (by VOICE_AGENT_NAME) using the provided API key.
 */
export async function findVoiceAgent(apiKey: string): Promise<FindAgentResult> {
    try {
        const response = await fetch(`${ELEVENLABS_API_BASE}/convai/agents`, {
            method: 'GET',
            headers: {
                'xi-api-key': apiKey,
                'Accept': 'application/json'
            }
        })

        if (!response.ok) {
            const errorData = await response.json().catch(() => ({})) as { detail?: { message?: string } | string }
            const errorMessage = typeof errorData.detail === 'string'
                ? errorData.detail
                : errorData.detail?.message || `API error: ${response.status}`
            return { success: false, error: errorMessage }
        }

        const data = await response.json() as { agents?: ElevenLabsAgent[] }
        const agents: ElevenLabsAgent[] = data.agents || []

        const voiceAgent = agents.find(agent => agent.name === VOICE_AGENT_NAME)

        if (voiceAgent) {
            return { success: true, agentId: voiceAgent.agent_id }
        } else {
            return { success: false, error: `No agent named "${VOICE_AGENT_NAME}" found` }
        }
    } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : 'Network error' }
    }
}

/**
 * Create or update the voice agent (by VOICE_AGENT_NAME) with our default configuration.
 */
export async function createOrUpdateVoiceAgent(apiKey: string): Promise<CreateAgentResult> {
    try {
        const findResult = await findVoiceAgent(apiKey)
        const existingAgentId = findResult.success ? findResult.agentId : null

        const agentConfig = buildVoiceAgentConfig()

        let response: Response
        let created = false

        if (existingAgentId) {
            response = await fetch(`${ELEVENLABS_API_BASE}/convai/agents/${existingAgentId}`, {
                method: 'PATCH',
                headers: {
                    'xi-api-key': apiKey,
                    'Content-Type': 'application/json',
                    'Accept': 'application/json'
                },
                body: JSON.stringify(agentConfig)
            })
        } else {
            response = await fetch(`${ELEVENLABS_API_BASE}/convai/agents/create`, {
                method: 'POST',
                headers: {
                    'xi-api-key': apiKey,
                    'Content-Type': 'application/json',
                    'Accept': 'application/json'
                },
                body: JSON.stringify(agentConfig)
            })
            created = true
        }

        if (!response.ok) {
            const errorData = await response.json().catch(() => ({})) as { detail?: { message?: string } | string }
            const errorMessage = typeof errorData.detail === 'string'
                ? errorData.detail
                : errorData.detail?.message || `API error: ${response.status}`
            return { success: false, error: errorMessage }
        }

        const data = await response.json() as { agent_id?: string }
        const agentId = existingAgentId || data.agent_id

        if (!agentId) {
            return { success: false, error: 'Failed to get agent ID from response' }
        }

        return { success: true, agentId, created }
    } catch (e) {
        return { success: false, error: e instanceof Error ? e.message : 'Network error' }
    }
}
