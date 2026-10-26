# A used file can use others: paths still start at the main script's dir.
use models::person::Person

class User < Person {
  fn init(name, @email) {
    super(name)
  }

  fn to_s = "#{name} <#{@email}>"
  fn admin? = false
}
