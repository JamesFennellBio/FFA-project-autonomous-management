cfnuse std::ptr;

fn main() {
    let a: &'static mut String = Box::leak(Box::new(String::from("hello, world")));
    println!("{}", a);

    let mut b = [0u8; 16];
    let c = [b'x'; 33];
    unsafe {
        ptr::copy_nonoverlapping(c.as_ptr(), b.as_mut_ptr(), c.len());
    }

    let d = 0xdead_beef_usize as *mut i32;
    unsafe {
        *d = 42;
    }
}
//scanner test program with intentional bugs
