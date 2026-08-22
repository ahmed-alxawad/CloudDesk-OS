<script lang="ts">
  import { onMount } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';

  // PASS SSH-C, Task 9/44: reuses the existing local TerminalApp.svelte's
  // xterm.js + binary-WebSocket wiring almost verbatim -- the only real
  // difference is the URL (a specific RemoteServer) and the extra
  // `revoked`/`exited` states this connection can report that the local
  // terminal never needs to.
  export let serverId: string;

  let container: HTMLDivElement;
  let status:
    | 'connecting'
    | 'connected'
    | 'exited'
    | 'error'
    | 'revoked'
    | 'disconnected' = 'connecting';
  let statusDetail = '';
  let socket: WebSocket | null = null;
  let terminal: Terminal | null = null;
  let fit: FitAddon | null = null;
  let observer: ResizeObserver | null = null;
  let disposed = false;
  let reconnectAttempts = 0;

  onMount(() => {
    terminal = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontFamily: "'JetBrains Mono', 'SFMono-Regular', Consolas, monospace",
      fontSize: 13,
      theme: {
        background: '#07121a',
        foreground: '#d5e5e7',
        cursor: '#70dde2',
        selectionBackground: '#4e99a466'
      }
    });
    fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(container);
    fit.fit();
    terminal.writeln('\x1b[36mCloudDesk remote terminal\x1b[0m');
    terminal.onData((data) => {
      if (socket?.readyState === WebSocket.OPEN)
        socket.send(new TextEncoder().encode(data));
    });
    observer = new ResizeObserver(() => {
      fit?.fit();
      sendResize();
    });
    observer.observe(container);
    connect();
    return () => {
      disposed = true;
      observer?.disconnect();
      if (socket?.readyState === WebSocket.OPEN)
        socket.send(JSON.stringify({ type: 'close' }));
      socket?.close();
      terminal?.dispose();
    };
  });

  function connect() {
    socket?.close();
    status = 'connecting';
    statusDetail = '';
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    socket = new WebSocket(
      `${protocol}//${window.location.host}/api/v1/remote/servers/${serverId}/terminal/ws`
    );
    socket.binaryType = 'arraybuffer';
    socket.onopen = () => {
      status = 'connected';
      reconnectAttempts = 0;
      terminal?.focus();
      sendResize();
    };
    socket.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        terminal?.write(new Uint8Array(event.data));
        return;
      }
      try {
        const message = JSON.parse(String(event.data));
        if (message.type === 'exit') {
          status = 'exited';
          statusDetail =
            message.code === null || message.code === undefined
              ? ''
              : `exit code ${message.code}`;
          terminal?.writeln(`\r\n[remote shell exited: ${statusDetail}]`);
        } else if (message.type === 'error') {
          status = 'error';
          statusDetail = message.message ?? '';
          terminal?.writeln(`\r\n[terminal error: ${statusDetail}]`);
        } else if (message.type === 'revoked') {
          // Task 18/19: RemoteServer revocation or CloudDesk session
          // revocation/logout, detected server-side and reported here --
          // never left as a silent hang.
          status = 'revoked';
          statusDetail = 'access to this server was revoked';
          terminal?.writeln(`\r\n[terminal closed: ${statusDetail}]`);
        }
      } catch {
        terminal?.write(String(event.data));
      }
    };
    socket.onerror = () => {
      if (status === 'connecting') status = 'error';
    };
    socket.onclose = (event) => {
      if (disposed) return;
      // A control message (exit/error/revoked) already explains this
      // close -- don't overwrite it with a generic "disconnected".
      if (status === 'exited' || status === 'error' || status === 'revoked')
        return;
      status = event.wasClean ? 'disconnected' : 'error';
      if (!event.wasClean && reconnectAttempts < 3) {
        reconnectAttempts += 1;
        window.setTimeout(connect, 500 * 2 ** reconnectAttempts);
      }
    };
  }

  function sendResize() {
    if (socket?.readyState !== WebSocket.OPEN || !terminal) return;
    socket.send(
      JSON.stringify({
        type: 'resize',
        rows: terminal.rows,
        cols: terminal.cols
      })
    );
  }
</script>

<section class="terminal-app">
  <header>
    <span class={`terminal-status ${status}`}></span><strong>{status}</strong
    >{#if statusDetail}<span>{statusDetail}</span>{/if}<span
      >Remote server session · audited</span
    >{#if status === 'error' || status === 'disconnected'}<button
        onclick={connect}>Reconnect</button
      >{/if}
  </header>
  <div class="terminal-surface" bind:this={container}></div>
</section>
