use crate::Result;
use comfy_table::Table;

pub fn print_table(table: Table) -> Result<()> {
    let table = table.to_string();
    for line in table.lines() {
        println!("{}", line.trim());
    }
    Ok(())
}
