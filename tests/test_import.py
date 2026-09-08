from importlib import import_module


def test_python_package_is_importable() -> None:
    module_name = "src"

    module = import_module(module_name)

    assert module.__name__ == module_name
