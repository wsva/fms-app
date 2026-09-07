// Longest Common Subsequence
export function lcs(a: string[], b: string[]) {
  const n = a.length,
    m = b.length;
  const dp = Array.from({ length: n + 1 }, () => Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      if (a[i] === b[j]) dp[i][j] = 1 + dp[i + 1][j + 1];
      else dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  let i = 0,
    j = 0;
  const matchesB2A: number[][] = [];
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      matchesB2A.push([j, i]);
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) i++;
    else j++;
  }
  return matchesB2A;
}
