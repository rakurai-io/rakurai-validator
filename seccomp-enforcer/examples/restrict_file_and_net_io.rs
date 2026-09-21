use seccomp_enforcer::apply_seccomp_filter_for_file_and_network_io;
use std::fs::File;
use std::io::Write;
use std::io::Read;
use std::thread;
use std::net::TcpStream;

fn main() {
    let file_io_thread = thread::spawn(|| {
        apply_seccomp_filter_for_file_and_network_io().expect("Failed to apply seccomp filter for file and network I/O");
        // create a file
        let mut file = File::create("hello-world.txt").expect("Failed to create file");
        // write to the file
        file.write_all(b"Hello, world!").expect("Failed to write to file");
        // read the file
        let mut content = String::new();
        file.read_to_string(&mut content).expect("Failed to read file");
    });

    match file_io_thread.join() {
        Ok(_) => println!("File I/O thread completed successfully"),
        Err(e) => {
            if let Some(s) = e.downcast_ref::<String>() {
                eprintln!("File I/O thread panicked: {}", s);
            } else if let Some(s) = e.downcast_ref::<&str>() {
                eprintln!("File I/O thread panicked: {}", s);
            } else {
                eprintln!("File I/O thread panicked with unknown error type {:?}", e);
            }
        }
    }

    let network_io_thread = thread::spawn(|| {
        apply_seccomp_filter_for_file_and_network_io().expect("Failed to apply seccomp filter for file and network I/O");
        // open a socket
        let mut socket = TcpStream::connect("127.0.0.1:8080").expect("Failed to connect to socket");
        // send a message
        socket.write_all(b"Hello, world!").expect("Failed to send message");
        // read the message
        let mut buffer = [0; 1024];
        socket.read(&mut buffer).expect("Failed to read message");
    });

    match network_io_thread.join() {
        Ok(_) => println!("Network I/O thread completed successfully"),
        Err(e) => {
            if let Some(s) = e.downcast_ref::<String>() {
                eprintln!("Network I/O thread panicked: {}", s);
            } else if let Some(s) = e.downcast_ref::<&str>() {
                eprintln!("Network I/O thread panicked: {}", s);
            } else {
                eprintln!("Network I/O thread panicked with unknown error type {:?}", e);
            }
        }
    }
}