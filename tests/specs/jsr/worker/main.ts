import { addAsync } from "jsr:@endetest/worker";

addAsync(1, 2).then((result) => {
  console.log(result);
});
