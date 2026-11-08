## 2024-11-08 - Explicit limits for Regex compiling
**Vulnerability:** Unbounded Regex size limit compilation.
**Learning:** `regex::Regex::new` allows processing without a bounding limit when constructed on user input. Even with the default regex limit, adding a length check and explicitly configuring size limit are beneficial for defense-in-depth against Regex DoS.
**Prevention:** Bound input length before passing it to compile complex regular expressions, and explicitly use `RegexBuilder` with a `.size_limit` option.
