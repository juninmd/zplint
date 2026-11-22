diff --git a/src/fix.rs b/src/fix.rs
index 1a2b3c4..5d6e7f8 100644
--- a/src/fix.rs
+++ b/src/fix.rs
@@ -17,7 +17,9 @@ pub fn decode_source(path: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
      let mut file = File::open(path)?;
      let mut contents = Vec::new();
      file.read_to_end(&mut contents)?;
-     let _ = String::from_utf8(contents).map_err(|e| {
+    let _ = String::from_utf8(contents).map_err(|e| {
         eprintln!("Failed to decode {} as UTF-8: {}", path.display(), e);
         e
     });
      Ok(contents)
  }