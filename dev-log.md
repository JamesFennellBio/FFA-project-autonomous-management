This project has potential, I have plenty of time but none to waste.
109 days to production: Figuring out initial logic and realizing that ECC is better than RSA for sped and key length. I will use seed phrases to allow for easy rever in case of hub node destruction

108:I am going to use ChaCha20 since i am realizing that the Pi has no hardware acceleration for AES or ECC. Range is a problem i will need to find a solution to and my current ideas are: bring my own router and use it as a propxty for the hub's comunications, use radio USB sticks (I feel like that is a thing), make a relay system with AO (always on) wall-powered signal repeaters, use a purpose built AP (access point) (probably too expenive).

107: Correction for 108 i am going to use ED25519 for signatures and ChaCha20 for the payload's encryption
