import { useCallback, useEffect, useRef, useState } from 'react'

type TerminalConnectionState =
    | { status: 'idle' }
    | { status: 'connecting' }
    | { status: 'connected' }
    | { status: 'error'; error: string }

type UseTerminalSocketOptions = {
    baseUrl: string
    token: string
    sessionId: string
    terminalId: string
}

type TerminalClientMessage =
    | { type: 'input'; data: string }
    | { type: 'resize'; cols: number; rows: number }

type TerminalServerMessage =
    | { type: 'output'; data: string }
    | { type: 'exit'; code: number }

function buildWsUrl(baseUrl: string, token: string, sessionId: string, terminalId: string): string {
    const url = new URL(baseUrl)
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    url.pathname = `/ws/terminal/${encodeURIComponent(sessionId)}/${encodeURIComponent(terminalId)}`
    url.search = new URLSearchParams({ token }).toString()
    return url.toString()
}

function safeSend(socket: WebSocket, payload: TerminalClientMessage): void {
    if (socket.readyState !== WebSocket.OPEN) {
        return
    }
    try {
        socket.send(JSON.stringify(payload))
    } catch {
        // Ignore send errors
    }
}

export function useTerminalSocket(options: UseTerminalSocketOptions): {
    state: TerminalConnectionState
    connect: (cols: number, rows: number) => void
    write: (data: string) => void
    resize: (cols: number, rows: number) => void
    disconnect: () => void
    onOutput: (handler: (data: string) => void) => void
    onExit: (handler: (code: number | null, signal: string | null) => void) => void
} {
    const [state, setState] = useState<TerminalConnectionState>({ status: 'idle' })
    const socketRef = useRef<WebSocket | null>(null)
    const outputHandlerRef = useRef<(data: string) => void>(() => {})
    const exitHandlerRef = useRef<(code: number | null, signal: string | null) => void>(() => {})
    const sessionIdRef = useRef(options.sessionId)
    const terminalIdRef = useRef(options.terminalId)
    const tokenRef = useRef(options.token)
    const baseUrlRef = useRef(options.baseUrl)
    const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null)
    const manualCloseRef = useRef(false)

    useEffect(() => {
        sessionIdRef.current = options.sessionId
        terminalIdRef.current = options.terminalId
        baseUrlRef.current = options.baseUrl
    }, [options.sessionId, options.terminalId, options.baseUrl])

    useEffect(() => {
        tokenRef.current = options.token
        if (!options.token) {
            socketRef.current?.close()
            socketRef.current = null
            setState({ status: 'idle' })
        }
    }, [options.token])

    const setErrorState = useCallback((message: string) => {
        setState({ status: 'error', error: message })
    }, [])

    const connect = useCallback((cols: number, rows: number) => {
        lastSizeRef.current = { cols, rows }
        const token = tokenRef.current
        const sessionId = sessionIdRef.current
        const terminalId = terminalIdRef.current

        if (!token || !sessionId || !terminalId) {
            setErrorState('Missing terminal credentials.')
            return
        }

        const existing = socketRef.current
        if (existing) {
            if (existing.readyState === WebSocket.OPEN) {
                safeSend(existing, { type: 'resize', cols, rows })
                setState({ status: 'connected' })
                return
            }
            if (existing.readyState === WebSocket.CONNECTING) {
                setState({ status: 'connecting' })
                return
            }
            socketRef.current = null
        }

        let url: string
        try {
            url = buildWsUrl(baseUrlRef.current, token, sessionId, terminalId)
        } catch {
            setErrorState('Invalid terminal URL.')
            return
        }

        manualCloseRef.current = false
        const socket = new WebSocket(url)
        socketRef.current = socket
        setState({ status: 'connecting' })

        socket.onopen = () => {
            if (socketRef.current !== socket) {
                return
            }
            setState({ status: 'connected' })
            const size = lastSizeRef.current
            if (size) {
                safeSend(socket, { type: 'resize', cols: size.cols, rows: size.rows })
            }
        }

        socket.onmessage = (event) => {
            if (socketRef.current !== socket) {
                return
            }
            if (typeof event.data !== 'string') {
                return
            }
            let parsed: unknown
            try {
                parsed = JSON.parse(event.data)
            } catch {
                return
            }
            if (!parsed || typeof parsed !== 'object' || typeof (parsed as { type?: unknown }).type !== 'string') {
                return
            }
            const msg = parsed as TerminalServerMessage
            if (msg.type === 'output' && typeof msg.data === 'string') {
                outputHandlerRef.current(msg.data)
            }
            if (msg.type === 'exit' && typeof msg.code === 'number') {
                exitHandlerRef.current(msg.code, null)
                setErrorState('Terminal exited.')
            }
        }

        socket.onerror = () => {
            if (socketRef.current !== socket) {
                return
            }
            setErrorState('Connection error')
        }

        socket.onclose = (event) => {
            if (socketRef.current !== socket) {
                return
            }
            socketRef.current = null
            if (manualCloseRef.current) {
                manualCloseRef.current = false
                setState({ status: 'idle' })
                return
            }
            const reason = event.reason || `code ${event.code}`
            setErrorState(`Disconnected: ${reason}`)
        }
    }, [setErrorState])

    const write = useCallback((data: string) => {
        const socket = socketRef.current
        if (!socket) {
            return
        }
        safeSend(socket, { type: 'input', data })
    }, [])

    const resize = useCallback((cols: number, rows: number) => {
        lastSizeRef.current = { cols, rows }
        const socket = socketRef.current
        if (!socket) {
            return
        }
        safeSend(socket, { type: 'resize', cols, rows })
    }, [])

    const disconnect = useCallback(() => {
        const socket = socketRef.current
        if (!socket) {
            return
        }
        manualCloseRef.current = true
        socket.close()
        socketRef.current = null
        setState({ status: 'idle' })
    }, [])

    const onOutput = useCallback((handler: (data: string) => void) => {
        outputHandlerRef.current = handler
    }, [])

    const onExit = useCallback((handler: (code: number | null, signal: string | null) => void) => {
        exitHandlerRef.current = handler
    }, [])

    return {
        state,
        connect,
        write,
        resize,
        disconnect,
        onOutput,
        onExit
    }
}
