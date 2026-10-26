# 07 — Files as modules: `use`
#
#   cargo run --release -- examples/07_use/main.rb
#
# Each file is its own namespace: what a file defines at its top level
# lives there, not everywhere. `use` imports names from another file; paths
# start at the main script's directory (`models::user` is models/user.rb).
# Every `use` is checked before the program runs: a missing file, a name the
# file doesn't define, or a cycle is a compile error.

use shapes::Shapes::{Circle, Square}    # classes inside a module of shapes.rb
use shapes::Units::*                    # everything in Units: constants and fns
use shapes::Text::shout                 # one function of a module
use models::user::User                  # models/user.rb

p Circle::new(2).area, Square::new(3).area, CM_PER_INCH
p to_cm(10), to_inches(2.54), shout("hi")   # imported functions, called bare
p Circle::name                           # the file is part of the full name

ada = User::new("Ada", "ada@example.org")
p ada.to_s, ada.admin?

# only what was named came in: neither the Shapes module nor SECRET
p defined?(Shapes::Circle), defined?(SECRET)
