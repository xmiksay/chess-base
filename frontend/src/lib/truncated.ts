// Shared wording for a long engine job cut off by the server's 5-minute budget
// (ADR-0054): the job returns what it finished and flags `truncated: true`.
export const TRUNCATED_NOTE = 'Stopped at the 5-minute engine limit — results are partial.'
