// Thin client for the chess-base backend JSON API.

async function getJson(path) {
  const res = await fetch(path)
  if (!res.ok) throw new Error(`${path} → ${res.status}`)
  return res.json()
}

export const api = {
  health: () => getJson('/api/health'),
}
