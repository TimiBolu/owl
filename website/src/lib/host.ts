/** Host as the visitor typed it. pubp keeps Host; X-Forwarded-Host wins if a proxy sets it. */
export function requestHost(headers: Headers): string {
  const raw = headers.get("x-forwarded-host") || headers.get("host") || "";
  return raw.split(",")[0].trim().replace(/:\d+$/, "").toLowerCase();
}
