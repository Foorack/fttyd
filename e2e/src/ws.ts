/** Raw websocket helpers for cases Playwright's page API cannot express. */

import { connect, type Socket } from "node:net";

export interface HandshakeResult {
  status: number;
}

/**
 * Performs a websocket handshake over TCP and reports the HTTP status. The socket is
 * closed immediately afterwards, so the server sees a short lived client.
 */
export async function handshake(
  port: number,
  options: { path?: string; origin?: string } = {},
): Promise<HandshakeResult> {
  const { path = "/ws", origin } = options;
  const key = Buffer.from(
    Array.from({ length: 16 }, () => Math.floor(Math.random() * 256)),
  ).toString("base64");
  const lines = [
    `GET ${path} HTTP/1.1`,
    `Host: 127.0.0.1:${port}`,
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Key: ${key}`,
    "Sec-WebSocket-Version: 13",
    "Sec-WebSocket-Protocol: tty",
  ];
  if (origin) lines.push(`Origin: ${origin}`);
  const request = `${lines.join("\r\n")}\r\n\r\n`;

  const { status, socket } = await open(port, request);
  socket.destroy();
  return { status };
}

/**
 * Holds a handshake open while a second one is attempted, which is how `--max-clients`
 * and `--once` refuse extra clients.
 */
export async function handshakeWhileHeld(
  port: number,
  second: { origin?: string } = {},
): Promise<{ first: number; second: number }> {
  const key = Buffer.from(
    Array.from({ length: 16 }, () => Math.floor(Math.random() * 256)),
  ).toString("base64");
  const request = [
    "GET /ws HTTP/1.1",
    `Host: 127.0.0.1:${port}`,
    "Upgrade: websocket",
    "Connection: Upgrade",
    `Sec-WebSocket-Key: ${key}`,
    "Sec-WebSocket-Version: 13",
    "Sec-WebSocket-Protocol: tty",
    "",
    "",
  ].join("\r\n");

  const held = await open(port, request);
  try {
    const secondResult = await handshake(port, second);
    return { first: held.status, second: secondResult.status };
  } finally {
    held.socket.destroy();
  }
}

function open(port: number, request: string): Promise<{ status: number; socket: Socket }> {
  return new Promise((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port });
    let buffer = "";
    const timer = setTimeout(
      () => reject(new Error(`handshake timed out: ${buffer.slice(0, 120)}`)),
      5000,
    );
    socket.on("data", (chunk) => {
      buffer += chunk.toString("latin1");
      const match = /^HTTP\/1\.1 (\d{3})/.exec(buffer);
      if (match) {
        clearTimeout(timer);
        resolve({ status: Number(match[1]), socket });
      }
    });
    socket.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    socket.write(request);
  });
}
