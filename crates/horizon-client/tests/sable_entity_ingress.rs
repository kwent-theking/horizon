use horizon_client::sable::resident_entity_flag;
use horizon_protocol::{
    buf::Writer,
    connection::{Connection, Pipe},
    packets::play::{Clientbound, MoveEntityPos, MoveEntityPosRot, TeleportEntity, relative},
    translation::{self, p767},
};

// This integration binary owns its translation state. Exercise the same
// mapped-id / original-body hook as net::receive_play, through real framing.
#[test]
fn sable_phase_5c_entity_suffixes_at_767_ingress() {
    let (client, wire) = Pipe::pair();
    let (mut reader, _) = Connection::local(client).split();
    let (_, mut wire) = Connection::local(wire).split();
    reader
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let entity = 300;
    let position = [12.25, 70.0, -4.5];
    let delta = [4096, 0, -2048];
    translation::activate(p767::translation());

    // 1.21.1 Pos, PosRot and TeleportEntity, with Sable's writeBoolean
    // after onGround. Entity id, delta, angles and position are fixture inputs.
    for wire_id in [46, 47, 112] {
        let mut body = Writer::new();
        body.varint(wire_id);
        body.varint(entity);
        if wire_id == 112 {
            for component in position {
                body.f64(component);
            }
        } else {
            for component in delta {
                body.i16(component);
            }
        }
        if wire_id != 46 {
            body.i8(64);
            body.i8(-32);
        }
        body.bool(true);
        for flag in [Some(false), Some(true), None] {
            let mut bytes = body.data.clone();
            if let Some(flag) = flag {
                bytes.push(u8::from(flag));
            }
            wire.send_raw(&bytes).unwrap();
            let ((id, payload), actual) = reader.recv_with_wire(resident_entity_flag).unwrap();
            assert_eq!(actual, flag);
            match Clientbound::decode(id, &payload).unwrap() {
                Clientbound::MoveEntityPos(MoveEntityPos(p))
                | Clientbound::MoveEntityPosRot(MoveEntityPosRot(p)) => {
                    assert_eq!(p.id, entity);
                    assert!(p.on_ground);
                    assert_eq!(p.delta.unwrap().end(position), [13.25, 70.0, -5.0]);
                    assert_eq!(p.rotation, (wire_id == 47).then_some((64, -32)));
                }
                Clientbound::TeleportEntity(p) => {
                    assert_eq!(p.id, entity);
                    assert!(p.on_ground);
                    assert_eq!(p.position, position);
                    assert_eq!(p.relatives & (relative::X | relative::Y | relative::Z), 0);
                    assert_eq!((p.yaw, p.pitch), (90.0, -45.0));
                }
                packet => panic!("unexpected translated packet {packet:?}"),
            }
        }
    }
    translation::deactivate();

    // A native 777 packet keeps its relative flags and has no 767 metadata.
    let native = Clientbound::TeleportEntity(TeleportEntity {
        id: entity,
        position,
        delta: [0.0; 3],
        yaw: 90.0,
        pitch: -45.0,
        relatives: relative::X | relative::Z,
        on_ground: true,
    });
    wire.send_raw(&native.encode()).unwrap();
    let ((id, payload), flag) = reader.recv_with_wire(resident_entity_flag).unwrap();
    assert_eq!(flag, None);
    assert_eq!(Clientbound::decode(id, &payload).unwrap(), native);
}
