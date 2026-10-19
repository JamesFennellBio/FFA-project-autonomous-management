match first_byte {
  0x00 => data_fn
  0x01 => message_fn
  0x02 => alert_fn
}
fn data_fn(raw: [u8; 64] -> [u8;64]) {
  fn nonce_slicer(slice: [u8; 64] -> [u8; 12]) {
    let nonce_slice = slice[0...11];
    return nonce_slice
    }
fn 


}
