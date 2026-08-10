use super::*;
use crate::types::{CellType, CellValue};

#[test]
fn test_vec_storage_basic() {
    let mut storage = VecStorage {
        cells: vec![Cell::default(); 4],
        width: 2,
        height: 2,
    };
    assert_eq!(storage.get(0, 0).unwrap().value, CellValue(CellType(0)));
    storage.set(
        0,
        0,
        Cell {
            value: CellValue(CellType(5)),
            born_at: 0,
        },
    );
    assert_eq!(storage.get(0, 0).unwrap().value, CellValue(CellType(5)));
}

#[test]
fn test_vec_storage_out_of_bounds() {
    let storage = VecStorage {
        cells: vec![Cell::default(); 4],
        width: 2,
        height: 2,
    };
    assert!(storage.get(5, 5).is_none());
}

#[test]
fn test_chunk_storage_basic() {
    let mut storage = ChunkStorage::new();
    assert_eq!(storage.width(), usize::MAX);
    assert_eq!(storage.height(), usize::MAX);
    // Default cell
    assert_eq!(storage.get(0, 0).unwrap().value, CellValue(CellType(0)));
    // Set and get
    storage.set(
        10,
        20,
        Cell {
            value: CellValue(CellType(5)),
            born_at: 0,
        },
    );
    assert_eq!(storage.get(10, 20).unwrap().value, CellValue(CellType(5)));
    // Active cells
    let active: Vec<_> = storage.active_cells().collect();
    assert!(
        active.contains(&(10, 20)),
        "Active cells should include (10, 20)"
    );
}

#[test]
fn test_chunk_storage_get_mut_existing_cell() {
    let mut storage = ChunkStorage::new();
    // Сначала создаём ячейку через set() — единственный корректный способ
    storage.set(
        100,
        200,
        Cell {
            value: CellValue(CellType(42)),
            born_at: 0,
        },
    );
    assert_eq!(
        storage.get(100, 200).unwrap().value,
        CellValue(CellType(42))
    );
    let active: Vec<_> = storage.active_cells().collect();
    assert!(
        active.contains(&(100, 200)),
        "set should produce active cell"
    );
    // active_cells count should be 1 since we set one cell
    assert_eq!(active.len(), 1);
}

#[test]
fn test_chunk_storage_set_preserves_count() {
    let mut storage = ChunkStorage::new();
    // Устанавливаем не-дефолтную ячейку
    storage.set(
        0,
        0,
        Cell {
            value: CellValue(CellType(42)),
            born_at: 0,
        },
    );
    assert_eq!(storage.active_cells().count(), 1);

    // Меняем на дефолт
    storage.set(0, 0, Cell::default());
    assert_eq!(storage.active_cells().count(), 0);

    // Снова устанавливаем
    storage.set(
        0,
        0,
        Cell {
            value: CellValue(CellType(7)),
            born_at: 0,
        },
    );
    assert_eq!(storage.active_cells().count(), 1);
}

#[test]
fn test_vec_storage_bounds() {
    let storage = VecStorage {
        cells: vec![Cell::default(); 4],
        width: 2,
        height: 2,
    };
    assert_eq!(storage.bounds(), Some((2, 2)));
}

#[test]
fn test_chunk_storage_bounds() {
    let storage = ChunkStorage::new();
    assert_eq!(storage.bounds(), None);
}

#[test]
fn test_chunk_storage_active_cells_after_set_default() {
    let mut storage = ChunkStorage::new();
    storage.set(
        5,
        5,
        Cell {
            value: CellValue(CellType(99)),
            born_at: 0,
        },
    );
    assert_eq!(storage.active_cells().count(), 1);

    // set back to default
    storage.set(5, 5, Cell::default());
    assert_eq!(storage.active_cells().count(), 0);
}

#[test]
fn test_chunk_storage_iter_active_respects_chunk_bounds() {
    let mut storage = ChunkStorage::new();
    // Ячейка в первом чанке (0..64, 0..64)
    storage.set(10, 10, Cell { value: CellValue(CellType(1)), born_at: 0 });
    // Ячейка во втором чанке (64..128, 64..128)
    storage.set(100, 100, Cell { value: CellValue(CellType(2)), born_at: 0 });
    // Ячейка на границе чанков
    storage.set(63, 63, Cell { value: CellValue(CellType(3)), born_at: 0 });

    let mut active: Vec<_> = storage.active_cells().collect();
    active.sort();

    assert_eq!(active.len(), 3, "All three cells should be active");
    assert!(active.contains(&(10, 10)), "Chunk 0,0");
    assert!(active.contains(&(100, 100)), "Chunk 1,1");
    assert!(active.contains(&(63, 63)), "Chunk boundary");
}

#[test]
fn test_chunk_storage_chunk_isolation() {
    let mut storage = ChunkStorage::new();

    // Записываем ячейку на границе двух чанков
    // Чанк (0,0): x=63, y=63 (последняя ячейка первого чанка)
    storage.set(63, 63, Cell { value: CellValue(CellType(10)), born_at: 0 });

    // Проверяем, что соседние чанки не затронуты
    // (64, 63) — уже следующий чанк по x, должен быть default
    assert_eq!(
        storage.get(64, 63).unwrap().value,
        CellValue(CellType(0)),
        "Cell (64, 63) should be default (different chunk)"
    );

    // (63, 64) — следующий чанк по y, должен быть default
    assert_eq!(
        storage.get(63, 64).unwrap().value,
        CellValue(CellType(0)),
        "Cell (63, 64) should be default (different chunk)"
    );

    // Ячейка (63, 63) должна сохранить значение
    assert_eq!(
        storage.get(63, 63).unwrap().value,
        CellValue(CellType(10)),
        "Cell (63, 63) should retain its value"
    );
}

#[test]
fn test_chunk_storage_get_unloaded_chunk() {
    let storage = ChunkStorage::new();
    // Чтение из незагруженного чанка должно вернуть default
    assert_eq!(
        storage.get(1000, 2000).unwrap().value,
        CellValue(CellType(0)),
        "Unloaded chunk should return default cell"
    );
}

#[test]
fn test_chunk_storage_remove_cell() {
    let mut storage = ChunkStorage::new();

    // Устанавливаем ячейку
    storage.set(42, 42, Cell { value: CellValue(CellType(7)), born_at: 0 });
    assert_eq!(storage.active_cells().count(), 1);

    // Удаляем — устанавливаем default
    storage.set(42, 42, Cell::default());
    assert_eq!(
        storage.get(42, 42).unwrap().value,
        CellValue(CellType(0)),
        "After removal, cell should be default"
    );
    assert_eq!(storage.active_cells().count(), 0, "No active cells after removal");
}

#[test]
fn test_chunk_storage_multiple_writes_same_cell() {
    let mut storage = ChunkStorage::new();

    storage.set(0, 0, Cell { value: CellValue(CellType(1)), born_at: 0 });
    storage.set(0, 0, Cell { value: CellValue(CellType(2)), born_at: 0 });
    storage.set(0, 0, Cell { value: CellValue(CellType(3)), born_at: 0 });

    assert_eq!(
        storage.get(0, 0).unwrap().value,
        CellValue(CellType(3)),
        "Last write should win"
    );
    assert_eq!(storage.active_cells().count(), 1, "Still exactly one active cell");
}

#[test]
fn test_chunk_storage_write_and_clear_chunk() {
    let mut storage = ChunkStorage::new();

    // Заполняем 3 ячейки в одном чанке
    storage.set(0, 0, Cell { value: CellValue(CellType(1)), born_at: 0 });
    storage.set(1, 0, Cell { value: CellValue(CellType(2)), born_at: 0 });
    storage.set(0, 1, Cell { value: CellValue(CellType(3)), born_at: 0 });
    assert_eq!(storage.active_cells().count(), 3);

    // Затираем всё дефолтом
    storage.set(0, 0, Cell::default());
    storage.set(1, 0, Cell::default());
    storage.set(0, 1, Cell::default());
    assert_eq!(
        storage.active_cells().count(),
        0,
        "Chunk should become empty after all cells set to default"
    );
    // Чанк должен быть удалён из HashMap (пустой чанк не хранится)
    assert_eq!(storage.chunks.len(), 0, "Empty chunk should be removed from HashMap");
}

#[test]
fn test_chunk_storage_with_chunk_size_splits_at_custom_boundary() {
    // chunk_size=4 (не 64 по умолчанию) — (0,0) и (4,0) обязаны попасть в
    // РАЗНЫЕ чанки, хотя с дефолтным размером оба сидели бы в одном и том же
    // чанке (0,0). Если бы `ensure_chunk`/`chunk_coords`/`local_coords`
    // где-то забыли о `self.chunk_size` и продолжали молча использовать
    // `DEFAULT_CHUNK_SIZE`, этот тест не заметил бы разницы — именно
    // поэтому проверка идёт по факту разбиения на чанки, а не только по
    // значениям get().
    let mut storage = ChunkStorage::with_chunk_size(4);
    storage.set(0, 0, Cell { value: CellValue(CellType(1)), born_at: 0 });
    storage.set(4, 0, Cell { value: CellValue(CellType(2)), born_at: 0 });

    assert_eq!(storage.get(0, 0).unwrap().value, CellValue(CellType(1)));
    assert_eq!(storage.get(4, 0).unwrap().value, CellValue(CellType(2)));
    assert_eq!(
        storage.chunks.len(),
        2,
        "с chunk_size=4 клетки (0,0) и (4,0) обязаны попасть в разные чанки"
    );

    // А (0,0) и (3,0) — ещё внутри одного и того же 4×4 чанка.
    storage.set(3, 0, Cell { value: CellValue(CellType(3)), born_at: 0 });
    assert_eq!(
        storage.chunks.len(),
        2,
        "(3,0) должна лечь в тот же чанк, что и (0,0) — размер чанка всё ещё 4"
    );
    assert_eq!(storage.get(3, 0).unwrap().value, CellValue(CellType(3)));

    let active: std::collections::HashSet<_> = storage.active_cells().collect();
    assert_eq!(active, std::collections::HashSet::from([(0, 0), (4, 0), (3, 0)]));
}
