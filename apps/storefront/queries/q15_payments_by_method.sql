-- Payments in one quarter, by method
SELECT method, COUNT(*) AS n, SUM(amount) AS amount
FROM payments WHERE paid_at >= '2026-01-01' AND paid_at < '2026-04-01'
GROUP BY method ORDER BY method ASC
