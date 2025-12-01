// Test Reference Navigation with Simplified Syntax
// Tests that `author: User` syntax works and enables O(1) navigation

use owl_db::{create_oql_runner, parse_oql};
use tempfile::TempDir;

#[test]
fn test_simple_reference_navigation() {
    let temp_dir = TempDir::new().unwrap();
    let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
    
    let oql = r#"
        schema User {
            id: String,
            name: String,
            email: String
        }
        
        schema Post {
            id: String,
            title: String,
            author: User
        }
        
        User.insert {
            id: "u1",
            name: "Alice",
            email: "alice@test.com"
        }
        
        User.insert {
            id: "u2",
            name: "Bob",
            email: "bob@test.com"
        }
        
        Post.insert {
            id: "p1",
            title: "First Post",
            author: "u1"
        }
        
        Post.insert {
            id: "p2",
            title: "Second Post",
            author: "u2"
        }
    "#;
    
    runner.execute_content(oql).unwrap();
    
    // Verify posts were created with references
    let executor = runner.executor();
    let posts_query = parse_oql("Post").unwrap();
    let posts = executor.execute_query(&posts_query).unwrap();
    
    assert_eq!(posts.len(), 2);
    
    // Check that author field is a Reference, not a String
    for post in &posts {
        let author_value = post.fields.get("author").expect("Post should have author field");
        assert!(matches!(author_value, owl_db::document::Value::Reference(_)), 
            "Author should be a Reference, got: {:?}", author_value);
    }
    
    // Test reference navigation
    let nav_query = parse_oql(r#"
        Post.select {
            id,
            title,
            author_name: .author.name,
            author_email: .author.email
        }
    "#).unwrap();
    
    let results = executor.execute_query(&nav_query).unwrap();
    assert_eq!(results.len(), 2);
    
    // Verify navigation worked
    for result in &results {
        let author_name = result.fields.get("author_name").expect("Should have author_name");
        let author_email = result.fields.get("author_email").expect("Should have author_email");
        
        // Should not be Null
        assert!(!author_name.is_null(), "author_name should not be null");
        assert!(!author_email.is_null(), "author_email should not be null");
        
        // Should be actual values
        assert!(matches!(author_name, owl_db::document::Value::String(_)));
        assert!(matches!(author_email, owl_db::document::Value::String(_)));
    }
}

#[test]
fn test_multi_hop_reference_navigation() {
    let temp_dir = TempDir::new().unwrap();
    let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
    
    let oql = r#"
        schema User {
            id: String,
            name: String
        }
        
        schema Post {
            id: String,
            title: String,
            author: User
        }
        
        schema Comment {
            id: String,
            text: String,
            post: Post
        }
        
        User.insert {
            id: "u1",
            name: "Alice"
        }
        
        Post.insert {
            id: "p1",
            title: "Great Post",
            author: "u1"
        }
        
        Comment.insert {
            id: "c1",
            text: "Nice post!",
            post: "p1"
        }
    "#;
    
    runner.execute_content(oql).unwrap();
    
    // Test multi-hop navigation: Comment -> Post -> User
    let executor = runner.executor();
    let nav_query = parse_oql(r#"
        Comment.select {
            id,
            text,
            post_title: .post.title,
            author_name: .post.author.name
        }
    "#).unwrap();
    
    let results = executor.execute_query(&nav_query).unwrap();
    assert_eq!(results.len(), 1);
    
    let result = &results[0];
    let post_title = result.fields.get("post_title").expect("Should have post_title");
    let author_name = result.fields.get("author_name").expect("Should have author_name");
    
    assert!(matches!(post_title, owl_db::document::Value::String(s) if s == "Great Post"));
    assert!(matches!(author_name, owl_db::document::Value::String(s) if s == "Alice"));
}

#[test]
fn test_optional_reference() {
    let temp_dir = TempDir::new().unwrap();
    let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
    
    let oql = r#"
        schema User {
            id: String,
            name: String
        }
        
        schema Post {
            id: String,
            title: String,
            author: User?
        }
        
        User.insert {
            id: "u1",
            name: "Alice"
        }
        
        Post.insert {
            id: "p1",
            title: "Post with author",
            author: "u1"
        }
        
        Post.insert {
            id: "p2",
            title: "Anonymous post"
        }
    "#;
    
    runner.execute_content(oql).unwrap();
    
    // Query posts
    let executor = runner.executor();
    let posts_query = parse_oql("Post").unwrap();
    let posts = executor.execute_query(&posts_query).unwrap();
    
    assert_eq!(posts.len(), 2);
    
    // One should have a reference, one should not
    let with_author = posts.iter().find(|p| {
        matches!(p.fields.get("author"), Some(owl_db::document::Value::Reference(_)))
    });
    
    let without_author = posts.iter().find(|p| {
        !matches!(p.fields.get("author"), Some(owl_db::document::Value::Reference(_)))
    });
    
    assert!(with_author.is_some(), "Should have post with author");
    assert!(without_author.is_some(), "Should have post without author");
}

#[test]
fn test_reference_not_found_error() {
    let temp_dir = TempDir::new().unwrap();
    let runner = create_oql_runner(temp_dir.path().join("test_db")).unwrap();
    
    let oql = r#"
        schema User {
            id: String,
            name: String
        }
        
        schema Post {
            id: String,
            title: String,
            author: User
        }
        
        Post.insert {
            id: "p1",
            title: "Orphan Post",
            author: "nonexistent"
        }
    "#;
    
    // Should fail because referenced user doesn't exist
    // The execution will succeed but the insert will fail
    // We can verify by checking that no posts were created
    runner.execute_content(oql).ok(); // Ignore result
    
    let executor = runner.executor();
    let posts_query = parse_oql("Post").unwrap();
    let posts = executor.execute_query(&posts_query).unwrap();
    
    // No posts should have been created because the insert failed
    assert_eq!(posts.len(), 0, "No posts should be created when reference doesn't exist");
}
