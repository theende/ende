// WebSocketCloseInfo has `closeCode`, as in the WebSocketStream spec.
export function close(stream: WebSocketStream): void {
  stream.close({ closeCode: 1000, reason: "done" });
}

export async function closedCode(
  stream: WebSocketStream,
): Promise<number | undefined> {
  const info: WebSocketCloseInfo = await stream.closed;
  return info.closeCode;
}

export const error: WebSocketError = new WebSocketError("closed", {
  closeCode: 4000,
  reason: "bye",
});
