-- Best-rated products with at least five reviews
SELECT product AS product_id, COUNT(*) AS reviews, AVG(rating) AS rating
FROM reviews GROUP BY product HAVING COUNT(*) >= 5
ORDER BY rating DESC, product_id ASC LIMIT 20
