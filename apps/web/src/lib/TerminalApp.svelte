<script lang="ts">
  import { onMount } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import '@xterm/xterm/css/xterm.css';

  let container: HTMLDivElement;
  let status: 'connecting' | 'connected' | 'disconnected' | 'failed' =
    'connecting';
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
    terminal.writeln('\x1b[36mCloudDesk secure terminal\x1b[0m');
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
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    socket = new WebSocket(
      `${protocol}//${window.location.host}/api/v1/terminal/ws`
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
        if (message.type === 'exit')
          terminal?.writeln(`\r\n[process exited: ${message.code}]`);
        if (message.type === 'error')
          terminal?.writeln(`\r\n[terminal error: ${message.message}]`);
      } catch {
        terminal?.write(String(event.data));
      }
    };
    socket.onerror = () => {
      status = 'failed';
    };
    socket.onclose = (event) => {
      if (disposed) return;
      status = event.wasClean ? 'disconnected' : 'failed';
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
    ><span>Mapped Linux identity · audited session</span
    >{#if status === 'failed' || status === 'disconnected'}<button
        onclick={connect}>Reconnect</button
      >{/if}
  </header>
  <div class="terminal-surface" bind:this={container}></div>
</section>
