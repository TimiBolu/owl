//! A deterministic storefront dataset.
//!
//! The same `orders` count always produces the same data, so the two
//! engines are loaded with identical rows and a result can be reproduced.

pub struct Customer {
    pub id: String,
    pub name: String,
    pub email: String,
    pub tier: &'static str,
    pub country: &'static str,
    pub joined: String,
    pub city: String,
    pub zip: String,
}

pub struct Category {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
}

pub struct Product {
    pub id: String,
    pub name: String,
    pub category: String,
    pub price: f64,
    pub stock: i64,
    pub embedding: [f32; 8],
}

pub struct Item {
    pub product: String,
    pub qty: i64,
    pub price: f64,
}

pub struct Order {
    pub id: String,
    pub customer: String,
    pub status: &'static str,
    pub placed: String,
    pub total: f64,
    pub items: Vec<Item>,
}

pub struct Payment {
    pub id: String,
    pub order: String,
    pub method: &'static str,
    pub amount: f64,
    pub paid_at: String,
}

pub struct Review {
    pub id: String,
    pub product: String,
    pub customer: String,
    pub rating: i64,
    pub body: String,
    pub written: String,
}

pub struct Ticket {
    pub id: String,
    pub customer: String,
    pub order: Option<String>,
    pub status: &'static str,
    pub priority: i64,
    pub opened: String,
}

pub struct Dataset {
    pub customers: Vec<Customer>,
    pub categories: Vec<Category>,
    pub products: Vec<Product>,
    pub orders: Vec<Order>,
    pub payments: Vec<Payment>,
    pub reviews: Vec<Review>,
    pub tickets: Vec<Ticket>,
}

impl Dataset {
    pub fn rows(&self) -> usize {
        self.customers.len()
            + self.categories.len()
            + self.products.len()
            + self.orders.len()
            + self.payments.len()
            + self.reviews.len()
            + self.tickets.len()
    }
}

/// SplitMix64: small, fast, and identical on every platform.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn between(&mut self, low: i64, high: i64) -> i64 {
        low + (self.next() % (high - low + 1) as u64) as i64
    }

    /// Choose from weighted options; weights are out of 100.
    fn weighted<T: Copy>(&mut self, options: &[(T, u64)]) -> T {
        let mut roll = self.next() % 100;
        for (option, weight) in options {
            if roll < *weight {
                return *option;
            }
            roll -= weight;
        }
        options[options.len() - 1].0
    }
}

const COUNTRIES: [&str; 8] = ["NG", "GH", "KE", "ZA", "US", "GB", "DE", "IN"];
const CITIES: [&str; 8] = [
    "Lagos",
    "Accra",
    "Nairobi",
    "Cape Town",
    "Austin",
    "Leeds",
    "Berlin",
    "Pune",
];
const WORDS: [&str; 12] = [
    "solid", "quick", "fragile", "bright", "heavy", "quiet", "sharp", "soft", "sturdy", "plain",
    "handy", "neat",
];

/// 2025-01-01T00:00:00Z.
const EPOCH: i64 = 1_735_689_600;
const DAY: i64 = 86_400;

fn timestamp(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .expect("generated timestamps are in range")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

pub fn generate(order_count: usize) -> Dataset {
    let mut random = Random(0x0B5E_55ED ^ order_count as u64);
    let customer_count = (order_count / 10).max(20);
    let product_count = (order_count / 50).max(200);

    let customers: Vec<Customer> = (0..customer_count)
        .map(|index| {
            let place = random.below(COUNTRIES.len());
            Customer {
                id: format!("c{index:07}"),
                name: format!("Customer {index}"),
                email: format!("c{index}@example.com"),
                tier: random.weighted(&[("free", 70), ("silver", 20), ("gold", 10)]),
                country: COUNTRIES[place],
                // Two years of sign-ups before the order window opens.
                joined: timestamp(EPOCH - random.between(0, 730 * DAY)),
                city: CITIES[place].to_string(),
                zip: format!("{:05}", random.below(100_000)),
            }
        })
        .collect();

    // Six departments, each with four categories.
    let mut categories = Vec::new();
    let mut leaves = Vec::new();
    for root in 0..6 {
        let root_id = format!("dept-{root}");
        categories.push(Category {
            id: root_id.clone(),
            name: format!("Department {root}"),
            parent: None,
        });
        for child in 0..4 {
            let id = format!("cat-{root}-{child}");
            leaves.push(id.clone());
            categories.push(Category {
                id,
                name: format!("Category {root}.{child}"),
                parent: Some(root_id.clone()),
            });
        }
    }

    let products: Vec<Product> = (0..product_count)
        .map(|index| {
            let mut embedding = [0f32; 8];
            for component in &mut embedding {
                *component = random.between(-1000, 1000) as f32 / 1000.0;
            }
            Product {
                id: format!("p{index:06}"),
                name: format!(
                    "{} item {index}",
                    WORDS[random.below(WORDS.len())]
                ),
                category: leaves[random.below(leaves.len())].clone(),
                price: random.between(100, 50_000) as f64 / 100.0,
                stock: if random.below(10) == 0 {
                    0
                } else {
                    random.between(1, 200)
                },
                embedding,
            }
        })
        .collect();

    let mut orders = Vec::with_capacity(order_count);
    let mut payments = Vec::new();
    for index in 0..order_count {
        let placed_at = EPOCH + random.between(0, 545 * DAY);
        let mut cents = 0i64;
        let items: Vec<Item> = (0..random.between(1, 4))
            .map(|_| {
                let product = &products[random.below(products.len())];
                let qty = random.between(1, 5);
                cents += qty * (product.price * 100.0).round() as i64;
                Item {
                    product: product.id.clone(),
                    qty,
                    price: product.price,
                }
            })
            .collect();
        let status = random.weighted(&[
            ("paid", 70),
            ("open", 15),
            ("cancelled", 10),
            ("refunded", 5),
        ]);
        let order = Order {
            id: format!("o{index:08}"),
            customer: customers[random.below(customers.len())].id.clone(),
            status,
            placed: timestamp(placed_at),
            total: cents as f64 / 100.0,
            items,
        };
        if matches!(status, "paid" | "refunded") {
            payments.push(Payment {
                id: format!("pay{index:08}"),
                order: order.id.clone(),
                method: random.weighted(&[("card", 60), ("transfer", 25), ("wallet", 15)]),
                amount: order.total,
                paid_at: timestamp(placed_at + random.between(0, 3 * DAY)),
            });
        }
        orders.push(order);
    }

    let reviews: Vec<Review> = (0..order_count / 2)
        .map(|index| Review {
            id: format!("r{index:08}"),
            product: products[random.below(products.len())].id.clone(),
            customer: customers[random.below(customers.len())].id.clone(),
            rating: random.weighted(&[(5, 40), (4, 30), (3, 15), (2, 10), (1, 5)]),
            body: format!(
                "{} and {}",
                WORDS[random.below(WORDS.len())],
                WORDS[random.below(WORDS.len())]
            ),
            written: timestamp(EPOCH + random.between(0, 545 * DAY)),
        })
        .collect();

    let tickets: Vec<Ticket> = (0..order_count / 10)
        .map(|index| Ticket {
            id: format!("t{index:07}"),
            customer: customers[random.below(customers.len())].id.clone(),
            order: (random.below(10) < 6).then(|| orders[random.below(orders.len())].id.clone()),
            status: random.weighted(&[("closed", 75), ("open", 25)]),
            priority: random.between(1, 3),
            opened: timestamp(EPOCH + random.between(0, 545 * DAY)),
        })
        .collect();

    Dataset {
        customers,
        categories,
        products,
        orders,
        payments,
        reviews,
        tickets,
    }
}
