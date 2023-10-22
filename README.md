# Ridiculously incremental `memcached`

Do we need send and receive buffers in TCP sockets?

In principle the application could just be a state machine that reacts to new packets being available as they come in. For writing, we could first signal an intent to transmit, and only then obtain a buffer directly in the device's TX queue.

Here's the Socket API we're using:

```rust
pub trait Socket {
    fn receive<R>(&mut self, f: impl FnOnce(&[u8]) -> R) -> Option<R>;
    fn transmit<R>(&mut self, f: impl FnOnce(&mut [u8]) -> (usize, R)) -> Option<R>;
}
```

It's inspired by the [`Device` API in `smoltcp`](https://docs.rs/smoltcp/latest/smoltcp/phy/trait.Device.html).

In this repo I explore writing a very simplified `memcached`-like server in this style.

Only `GET` is implemented (`SET` is parsed, but not implemented). The code is already really bad. Essentially each state in the state machine has its own little buffer, and each state handling code repeats the usual buffer handling code (copying up to target buffer capacity etc.).

The receive side is really badly implemented, inspecting each character at a time. This guarantees correctness (in weird corner cases e.g. when a command is sent as many 1-byte packets), but is probably very slow in the common case where it's a single packet.

## It's worse than that

Bytes we write to a socket can end up on the wire multiple times in case of retransmission. This method doesn't handle that (unless Socket internally has its own buffer, which we want to avoid).

Could we do it with no buffer? Yes, but the application must be prepared to _rewind to a previous state_ in the state machine and run the sending steps again. And let's hope they are idempotent, otherwise things can get funny.

With simple use cases like streaming out a big blob of memory this could work. But complex protocol stuff (like `VALUE <key> <flags> <len>`, for example) is already too complicated, and would get worse.

Also: if we were to e.g. remember the `State` before each packet, this could easily use more memory than a simple buffer enough to hold the reply. (But note: only because of the `heapless::Vec` for the key. Had we just used a pointer, things would get easier, but we would have to share it between all the state instances. Maybe something like "sub-states" would work?)

## How to size the window

The TCP _window size_ is the number of bytes we're willing to receive. Normal TCP implementations use the free space in the recv buffer.

Consider reading a `GET` command. How many bytes do we want to receive? The answer is: up to the next newline. We don't want to process the next command until we handle that one. That is, pipelining requires a buffer to store the pipelined commands in while we handle the previous ones.

We could disallow pipelining. Then we set the window size to anything really (maybe the maximum expected command size). But then after parsing a command we have to discard the rest of the received data, because it's essentially invalid (it _would_ be a pipelined command if it was allowed).
