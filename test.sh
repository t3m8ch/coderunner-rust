#!/bin/bash

grpcurl -plaintext -import-path . -proto ./proto/coderunnerapi.proto \
  -d '{
    "code": "#include <iostream>\nint main() { std::cout << \"Hello World\" << std::endl; return 0; }",
    "language": "GNU_CPP",
    "compilation_limits": {
      "memory_bytes": 536870912,
      "time_ms": 5000
    },
    "execution_limits": {
      "memory_bytes": 268435456,
      "time_ms": 1000,
      "pids_count": 1,
      "stdout_size_bytes": 65536,
      "stderr_size_bytes": 65536
    },
    "test_data": [
      {
        "stdin": "",
        "stdout": "Hello World\n",
        "stderr": ""
      }
    ]
  }' \
  localhost:50051 coderunnerapi.TestingService/SubmitCode
