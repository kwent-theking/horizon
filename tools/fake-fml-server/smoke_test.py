# Smoke test for fake-fml-server: status + login + configuration + play keep-alive.
# Смок-тест fake-fml-server: status + login + configuration + play keep-alive.
# Usage: python smoke_test.py [port]
import socket, sys, time, struct

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 25599

def varint(v):
    out = b""
    while True:
        b = v & 0x7F
        v >>= 7
        if v:
            out += bytes([b | 0x80])
        else:
            out += bytes([b])
            return out

def read_varint(sock):
    n, shift = 0, 0
    while True:
        b = sock.recv(1)[0]
        n |= (b & 0x7F) << shift
        shift += 7
        if not (b & 0x80):
            return n

def read_packet(sock):
    total = read_varint(sock)
    body = b""
    while len(body) < total:
        chunk = sock.recv(total - len(body))
        if not chunk:
            raise EOFError("server closed")
        body += chunk
    pid = read_varint_body(body)
    idlen = len(varint(pid))
    return pid, body[idlen:]

def read_varint_body(body):
    n, shift = 0, 0
    i = 0
    while True:
        b = body[i]
        n |= (b & 0x7F) << shift
        shift += 7
        i += 1
        if not (b & 0x80):
            return n

def send(sock, pid, payload):
    body = varint(pid) + payload
    sock.sendall(varint(len(body)) + body)

def handshake(sock, intent):
    payload = varint(767) + varint(len("localhost")) + b"localhost" + struct.pack(">H", PORT) + varint(intent)
    send(sock, 0x00, payload)

sock = socket.create_connection(("127.0.0.1", PORT), timeout=10)

# --- status ---
handshake(sock, 1)
send(sock, 0x00, b"")
pid, data = read_packet(sock)
print("status response id=0x%02x len=%d" % (pid, len(data)))
# skip string length varint
slen = read_varint_body(data)
json_txt = data[len(varint(slen)):len(varint(slen)) + slen].decode()
print("STATUS JSON:", json_txt)
send(sock, 0x01, struct.pack(">q", 12345))
pid, data = read_packet(sock)
assert pid == 0x01 and data == struct.pack(">q", 12345), (pid, data)
print("pong ok")
sock.close()

# --- login ---
sock = socket.create_connection(("127.0.0.1", PORT), timeout=10)
handshake(sock, 2)
name = b"TestPlayer"
uuid = bytes.fromhex("069a79f444e94726a5befca90e38aaf5")
hello = varint(len(name)) + name + uuid
send(sock, 0x00, hello)

pid, data = read_packet(sock)
print("login finished id=0x%02x len=%d" % (pid, len(data)))
assert pid == 0x02
pos = 16  # uuid
nlen = read_varint_body(data[pos:])
echo_name = data[pos + len(varint(nlen)):pos + len(varint(nlen)) + nlen].decode()
assert echo_name == "TestPlayer", echo_name
print("login echo ok:", echo_name)

send(sock, 0x03, b"")  # LoginAcknowledged (serverbound)
print("login acknowledged sent")

pid, data = read_packet(sock)
print("select known packs id=0x%02x len=%d" % (pid, len(data)))
assert pid == 0x0E and read_varint_body(data) == 0
send(sock, 0x0E, varint(0))  # reply: 0 packs

# brand + fml payload
pid, data = read_packet(sock)
assert pid == 0x01
ch_len = read_varint_body(data)
ch = data[len(varint(ch_len)):len(varint(ch_len)) + ch_len].decode()
print("custom payload channel:", ch)
assert ch == "minecraft:brand"
pid, data = read_packet(sock)
ch_len = read_varint_body(data)
ch = data[len(varint(ch_len)):len(varint(ch_len)) + ch_len].decode()
print("custom payload channel:", ch)
assert ch == "neoforge:handshake"
# reply with client brand then ACK
brand = b"vanilla"
send(sock, 0x02, varint(len("minecraft:brand")) + b"minecraft:brand" + varint(len(brand)) + brand)
send(sock, 0x0C, b"")  # ACK
pid, data = read_packet(sock)
assert pid == 0x0D, hex(pid)
print("finish configuration ok")
pid, data = read_packet(sock)
assert pid == 0x00, hex(pid)
print("play login packet ok")
pid, data = read_packet(sock)
assert pid == 0x26 and len(data) == 8, (hex(pid), len(data))
ka = struct.unpack(">q", data)[0]
print("keep-alive", ka)
send(sock, 0x1B, data)
sock.close()
print("SMOKE TEST PASSED")
